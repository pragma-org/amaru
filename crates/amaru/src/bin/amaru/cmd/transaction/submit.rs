// Copyright 2026 PRAGMA
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::{fs, path::PathBuf, time::Duration};

use amaru::lifecycle::{Runnable, RuntimeKind, ShutdownHandle};
use amaru_kernel::{NetworkName, Point, ProtocolVersion, Transaction, TransactionId, ValidityInterval, cbor};
use amaru_metrics::Meter;
use amaru_node::{
    CallError, LedgerBlockEvent, LedgerObservers, MempoolError, MempoolServices, TxInsertResult, TxRejectReason,
};
use anyhow::{Context, Result, bail, ensure};
use reqwest::{Client, StatusCode, Url, redirect::Policy};
use tokio::{sync::watch, time::Instant};

use crate::cmd::temporary_node;

/// Submit signed CBOR to a running node or start a temporary node with --wait.
#[derive(Debug, clap::Args)]
pub(crate) struct SubmitArgs {
    /// Signed transaction in binary CBOR.
    #[arg(long, env = "AMARU_SUBMIT_TRANSACTION", value_name = amaru::value_names::FILEPATH)]
    transaction: PathBuf,

    /// Full URL of a running node's transaction submit endpoint.
    #[arg(
        long,
        env = "AMARU_SUBMIT_URL",
        value_name = amaru::value_names::URL,
        required_unless_present = "wait",
        conflicts_with = "wait",
    )]
    submit_url: Option<Url>,

    /// Target Cardano network. Required with --wait.
    #[arg(long, env = amaru::env_vars::NETWORK, value_name = amaru::value_names::NETWORK)]
    network: Option<NetworkName>,

    /// Upstream peers for --wait. Defaults to the network's embedded peers.
    #[arg(
        long,
        alias = "peer-address",
        env = amaru::env_vars::PEER,
        value_name = amaru::value_names::ENDPOINT,
        value_delimiter = ',',
        num_args = 0..,
        help_heading = "Peers options",
    )]
    peer: Vec<String>,

    /// Start a temporary node and wait for confirmations. The inclusion block counts as one.
    #[arg(
        long,
        env = "AMARU_SUBMIT_WAIT",
        value_name = "BLOCKS",
        num_args = 0..=1,
        default_missing_value = "1",
        value_parser = clap::value_parser!(u64).range(1..),
        requires = "network",
    )]
    wait: Option<u64>,

    /// Timeout in seconds. Defaults to 30 for HTTP submission and 10800 with --wait.
    #[arg(
        long,
        env = "AMARU_SUBMIT_TIMEOUT",
        value_name = "SECONDS",
        value_parser = clap::value_parser!(u64).range(1..),
    )]
    timeout: Option<u64>,

    #[command(flatten, next_help_heading = "Storage options")]
    db_chain: amaru::args::DbChain,

    #[command(flatten, next_help_heading = "Storage options")]
    db_ledger: amaru::args::DbLedger,
}

impl SubmitArgs {
    pub(super) fn into_runnable(self) -> Runnable {
        let runtime = if self.wait.is_some() { RuntimeKind::Node } else { RuntimeKind::Io };
        Runnable::soft(runtime, move |shutdown, meter| self.run(shutdown, meter))
    }

    #[expect(clippy::print_stdout)]
    async fn run(self, shutdown: ShutdownHandle, meter: Meter) -> Result<()> {
        let bytes = fs::read(&self.transaction).with_context(|| format!("reading {}", self.transaction.display()))?;
        let tx: Transaction = cbor::from_cbor_no_leftovers_with(&bytes, &mut ProtocolVersion::default())?;
        let id = tx.tx_id();
        if let Some(network) = self.network {
            ensure!(tx.body.network.is_none_or(|n| n == network.into()), "transaction is for a different network");
        }
        let cancel = shutdown.token();
        let default_timeout = if self.wait.is_some() { 10800 } else { 30 };
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(self.timeout.unwrap_or(default_timeout)))
            .context("timeout is too large")?;
        if let Some(url) = self.submit_url {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => bail!("transaction submission cancelled"),
                _ = tokio::time::sleep_until(deadline) => bail!("timed out submitting transaction {id}"),
                result = post(url, &bytes, id) => result?,
            }
            println!("{}", serde_json::json!({"transaction_id": id}));
            return Ok(());
        }

        let confirmations = self.wait.context("provide --wait or --submit-url")?;
        let network = self.network.context("--wait requires --network")?;
        let ledger = self.db_ledger.into_path_buf(network);
        let chain = self.db_chain.into_path_buf(network);
        let builder = temporary_node::builder(network, ledger, chain, self.peer, meter)?;
        let validity = tx.body.validity_interval();
        let state = watch::Sender::new(Status::default());
        let observed = state.clone();
        let observer = LedgerObservers::new().on_block(move |event| {
            observed.send_modify(|s| match event {
                LedgerBlockEvent::Adopted(block) => {
                    s.tip = Some(block.point);
                    if s.submission_started
                        && let Some(tx) = block.transactions().find(|tx| tx.tx_id() == id)
                    {
                        s.inclusion = Some((block.point, tx.is_expected_valid));
                    }
                }
                LedgerBlockEvent::Undone(block) => {
                    s.tip = None;
                    s.fork += 1;
                    if s.inclusion.is_some_and(|(point, _)| point == block.point) {
                        s.inclusion = None;
                    }
                }
            });
        });
        let running = temporary_node::start(builder.observers(observer)).await?;
        state.send_modify(|s| {
            if s.tip.is_none() && s.fork == 0 {
                s.tip = Some(running.initial_ledger_tip());
            }
        });
        let mempool = running.mempool();
        let result = tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(anyhow::anyhow!("transaction submission cancelled")),
            _ = running.termination() => Err(anyhow::anyhow!("temporary node stopped during submission")),
            _ = tokio::time::sleep_until(deadline) => Err(anyhow::anyhow!("timed out waiting for confirmation of {id}")),
            result = submit_until_confirmed(id, network, validity, confirmations, &state, || submit_to_mempool(&mempool, &bytes, id)) => result,
        };
        let report = running.shutdown().await?;
        ensure!(report.is_clean(), "temporary node cleanup failed: {:?}", report.unexpected_exits);
        let (block, depth) = result?;
        println!("{}", serde_json::json!({"transaction_id": id, "block": block, "confirmations": depth}));
        Ok(())
    }
}

#[derive(Clone, Copy, Default)]
struct Status {
    fork: u64,
    tip: Option<Point>,
    inclusion: Option<(Point, bool)>,
    submission_started: bool,
}

async fn submit_until_confirmed(
    id: TransactionId,
    network: NetworkName,
    validity: ValidityInterval,
    confirmations: u64,
    state: &watch::Sender<Status>,
    mut submit: impl AsyncFnMut() -> Result<bool>,
) -> Result<(Point, u64)> {
    let mut status = state.subscribe();
    let mut retry = Instant::now();
    let mut submitted_on = None;
    loop {
        let current = *status.borrow_and_update();
        if let Some(tip) = current.tip
            && temporary_node::live(network, tip)?
        {
            if let Some((point, valid)) = current.inclusion {
                ensure!(valid, "transaction {id} failed script validation in block {point}");
                let depth = tip.block_height().into_u64() - point.block_height().into_u64() + 1;
                if depth >= confirmations {
                    return Ok((point, depth));
                }
                retry = Instant::now();
            } else {
                ensure!(
                    validity.upper_bound().is_none_or(|end| tip.slot() < end),
                    "transaction {id} expired before inclusion"
                );
                if submitted_on != Some(current.fork) && validity.includes(tip.slot()) && Instant::now() >= retry {
                    if !current.submission_started {
                        state.send_modify(|s| s.submission_started = true);
                    }
                    match submit().await {
                        Ok(true) => submitted_on = Some(current.fork),
                        Err(error) if status.borrow().inclusion.is_none() => return Err(error),
                        Ok(false) | Err(_) => (),
                    }
                    retry = Instant::now() + Duration::from_secs(10);
                }
            }
        }
        tokio::select! {
            result = status.changed() => result.context("ledger observer stopped")?,
            _ = tokio::time::sleep(Duration::from_secs(1)) => (),
        }
    }
}

/// Return whether the local mempool has the transaction, or false if submission can be retried.
async fn submit_to_mempool(mempool: &MempoolServices, bytes: &[u8], id: TransactionId) -> Result<bool> {
    match mempool.submit(bytes).await {
        Ok(TxInsertResult::Accepted { tx_id, .. }) => {
            ensure!(tx_id == id, "mempool returned a different transaction ID");
            Ok(true)
        }
        Ok(TxInsertResult::Rejected { reason: TxRejectReason::Duplicate, .. }) => Ok(true),
        Ok(TxInsertResult::Rejected { reason: TxRejectReason::MempoolFull, .. })
        | Err(MempoolError::Call(CallError::TimedOut)) => Ok(false),
        Ok(TxInsertResult::Rejected { reason, .. }) => bail!("mempool rejected {id}: {reason}"),
        Err(error) => Err(error).with_context(|| format!("submitting transaction {id} to the mempool")),
    }
}

async fn post(url: Url, bytes: &[u8], id: TransactionId) -> Result<()> {
    let client = Client::builder().redirect(Policy::none()).build()?;
    let response = client
        .post(url)
        .header("Content-Type", "application/cbor")
        .body(bytes.to_owned())
        .send()
        .await
        .map_err(reqwest::Error::without_url)
        .with_context(|| format!("sending transaction {id} to the submit API"))?;
    let code = response.status();
    let body = response.text().await?;
    match code {
        StatusCode::ACCEPTED => {
            ensure!(
                serde_json::from_str::<TransactionId>(&body)? == id,
                "submit API returned a different transaction ID"
            );
            Ok(())
        }
        StatusCode::CONFLICT => Ok(()),
        _ => bail!("submit API rejected {id} (HTTP {code}): {body}"),
    }
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use amaru_kernel::Hash;
    use tokio::time::timeout;

    use super::*;

    #[tokio::test]
    async fn starts_a_fresh_submission_after_rejection() -> Result<()> {
        let network = NetworkName::Preprod;
        let start = network.as_global_parameters().unwrap().system_start();
        let slot = network.as_era_history().unwrap().posix_time_to_slot(SystemTime::now(), start)?;
        let tip = Point::Specific(slot, Hash::new([1; 32]), 1.into());
        let id = TransactionId::new(Hash::new([2; 32]));
        let validity = ValidityInterval::default();
        let first = watch::Sender::new(Status { tip: Some(tip), ..Status::default() });
        let rejection = timeout(
            Duration::from_secs(5),
            submit_until_confirmed(id, network, validity, 1, &first, async || {
                assert!(first.borrow().submission_started);
                bail!("Unknown input")
            }),
        )
        .await?
        .unwrap_err();
        assert_eq!(rejection.to_string(), "Unknown input");

        let second = watch::Sender::new(Status { tip: Some(tip), ..Status::default() });
        let confirmation = timeout(
            Duration::from_secs(5),
            submit_until_confirmed(id, network, validity, 1, &second, async || {
                assert!(second.borrow().submission_started);
                second.send_modify(|s| s.inclusion = Some((tip, true)));
                Ok(true)
            }),
        )
        .await??;
        assert_eq!(confirmation, (tip, 1));
        Ok(())
    }
}
