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

use std::{
    fs,
    net::{Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    time::Duration,
};

use amaru::lifecycle::{Runnable, RuntimeKind, ShutdownHandle};
use amaru_kernel::{
    Block, IsHeader, NetworkName, Point, ProtocolVersion, Transaction, TransactionId, ValidityInterval, cbor,
};
use amaru_ledger::store::ReadStore;
use amaru_metrics::Meter;
use amaru_node::{LedgerBlockEvent, LedgerObservers, submit_api};
use amaru_ouroboros::BaseReadChainStore;
use amaru_stores::rocksdb::{ReadOnlyRocksDB, RocksDbConfig, consensus::RocksDBStore};
use anyhow::{Context, Result, bail, ensure};
use reqwest::{Client, StatusCode};
use tokio::{sync::watch, time::Instant};
use tokio_util::sync::CancellationToken;

use crate::cmd::temporary_node;

/// Submit signed CBOR through a temporary node's local HTTP API and wait for inclusion.
///
/// Uses bootstrapped chain and ledger stores exclusively. A retry checks retained
/// adopted history and resubmits the same signed bytes if needed.
#[derive(Debug, clap::Args)]
pub(crate) struct SubmitArgs {
    /// Signed transaction in binary CBOR.
    #[arg(long, env = "AMARU_SUBMIT_TRANSACTION", value_name = amaru::value_names::FILEPATH)]
    transaction: PathBuf,

    #[command(flatten)]
    network: amaru::args::Network,

    /// Upstream peers to synchronize from. Defaults to the network's embedded peers.
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

    /// Required confirmations. The inclusion block counts as the first confirmation.
    #[arg(
        long,
        env = "AMARU_SUBMIT_WAIT",
        value_name = "BLOCKS",
        num_args = 0..=1,
        default_value = "1",
        default_missing_value = "1",
        value_parser = clap::value_parser!(u64).range(1..),
    )]
    wait: u64,

    /// Maximum time in seconds to sync, submit and observe confirmation.
    #[arg(
        long,
        env = "AMARU_SUBMIT_TIMEOUT",
        default_value = "10800",
        value_name = "SECONDS",
        value_parser = clap::value_parser!(u64).range(1..),
    )]
    timeout: u64,

    #[command(flatten, next_help_heading = "Storage options")]
    db_chain: amaru::args::DbChain,

    #[command(flatten, next_help_heading = "Storage options")]
    db_ledger: amaru::args::DbLedger,
}

impl SubmitArgs {
    pub(super) fn into_runnable(self) -> Runnable {
        Runnable::soft(RuntimeKind::Node, move |shutdown, meter| self.run(shutdown, meter))
    }

    #[expect(clippy::print_stdout)]
    async fn run(self, shutdown: ShutdownHandle, meter: Meter) -> Result<()> {
        let bytes = fs::read(&self.transaction).with_context(|| format!("reading {}", self.transaction.display()))?;
        let tx: Transaction = cbor::from_cbor_no_leftovers_with(&bytes, &mut ProtocolVersion::default())?;
        let id = tx.tx_id();
        let network = NetworkName::from(self.network);
        ensure!(tx.body.network.is_none_or(|n| n == network.into()), "transaction is for a different network");
        let ledger = self.db_ledger.into_path_buf(network);
        let chain = self.db_chain.into_path_buf(network);
        let builder = temporary_node::builder(network, ledger.clone(), chain.clone(), self.peer, meter)?;
        let cancel = shutdown.token();
        let deadline = Instant::now().checked_add(Duration::from_secs(self.timeout)).context("timeout is too large")?;
        let validity = tx.body.validity_interval();
        let scan_cancel = cancel.clone();
        let prior =
            tokio::task::spawn_blocking(move || history(&ledger, &chain, &tx, deadline, &scan_cancel)).await??;
        let (state, rx) = watch::channel(prior);
        let observer = LedgerObservers::new().on_block(move |event| {
            state.send_modify(|s| match event {
                LedgerBlockEvent::Adopted(block) => {
                    s.tip = Some(block.point);
                    if let Some(valid) = included(block.block, id) {
                        s.inclusion = Some((block.point, valid));
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
        let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
        let server_cancel = CancellationToken::new();
        let result = async {
            let (server, addr) = submit_api::start(addr, running.mempool(), server_cancel.clone()).await?;
            let url = format!("http://{addr}/api/submit/tx");
            let result = tokio::select! {
                biased;
                _ = cancel.cancelled() => Err(anyhow::anyhow!("transaction submission cancelled")),
                _ = running.termination() => Err(anyhow::anyhow!("temporary node stopped during submission")),
                _ = tokio::time::sleep_until(deadline) => Err(anyhow::anyhow!("timed out waiting for confirmation of {id}")),
                result = submit_until_confirmed(&url, &bytes, id, network, validity, self.wait, rx) => result,
            };
            server_cancel.cancel();
            server.await?;
            result
        }.await;
        let report = running.shutdown().await?;
        ensure!(report.is_clean(), "temporary node cleanup failed: {:?}", report.unexpected_exits);
        let (block, depth) = result?;
        println!("{}", serde_json::json!({"transaction_id": id, "block": block, "confirmations": depth}));
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct Status {
    fork: u64,
    tip: Option<Point>,
    inclusion: Option<(Point, bool)>,
}

fn included(block: &Block, id: TransactionId) -> Option<bool> {
    block.into_iter().find_map(|(_, tx)| (tx.tx_id() == id).then_some(tx.is_expected_valid))
}

/// Volatile blocks are replayed by the node. Only previously stabilized history
/// needs looking up here. A still-unspent regular input rules out prior success.
fn history(
    ledger: &Path,
    chain: &Path,
    tx: &Transaction,
    deadline: Instant,
    cancel: &CancellationToken,
) -> Result<Status> {
    let db = ReadOnlyRocksDB::new(&RocksDbConfig::new(ledger.to_owned()))?;
    let store = RocksDBStore::open_for_readonly(&RocksDbConfig::new(chain.to_owned()))?;
    let mut point = store.load_point(&db.tip()?.hash()).context("ledger tip header not found in chain store")?;
    let mut state = Status { tip: Some(point), inclusion: None, fork: 0 };
    for input in tx.body.inputs.iter() {
        if db.utxo(input)?.is_some() && tx.body.collateral.is_none() {
            return Ok(state);
        }
    }
    let id = tx.tx_id();
    while point != Point::Origin && tx.body.validity_interval_start.is_none_or(|start| point.slot() >= start) {
        ensure!(!cancel.is_cancelled() && Instant::now() < deadline, "confirmation history scan interrupted");
        if tx.body.validity_interval().includes(point.slot())
            && let Some(raw) = store.load_block(&point.hash())?
            && let Some(valid) = included(&raw.decode()?, id)
        {
            state.inclusion = Some((point, valid));
            break;
        }
        let Some(parent) = store.load_header(&point.hash()).and_then(|header| header.parent()) else { break };
        let Some(previous) = store.load_point(&parent) else { break };
        ensure!(previous.slot() < point.slot(), "invalid local chain ancestry");
        point = previous;
    }
    Ok(state)
}

async fn submit_until_confirmed(
    url: &str,
    bytes: &[u8],
    id: TransactionId,
    network: NetworkName,
    validity: ValidityInterval,
    confirmations: u64,
    mut status: watch::Receiver<Status>,
) -> Result<(Point, u64)> {
    let client = Client::builder().no_proxy().timeout(Duration::from_secs(30)).build()?;
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
                    if post(&client, url, bytes, id).await? {
                        submitted_on = Some(current.fork);
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

/// True when the local mempool has the transaction. Transient failures can be retried.
async fn post(client: &Client, url: &str, bytes: &[u8], id: TransactionId) -> Result<bool> {
    let response = match client.post(url).header("Content-Type", "application/cbor").body(bytes.to_owned()).send().await
    {
        Ok(response) => response,
        Err(error) if error.is_timeout() || error.is_connect() => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    let code = response.status();
    let body = response.text().await?;
    match code {
        StatusCode::ACCEPTED => {
            ensure!(
                serde_json::from_str::<TransactionId>(&body)? == id,
                "submit API returned a different transaction ID"
            );
            Ok(true)
        }
        StatusCode::CONFLICT => Ok(true),
        StatusCode::SERVICE_UNAVAILABLE => Ok(false),
        StatusCode::BAD_REQUEST if body.to_ascii_lowercase().contains("missing transaction inputs") => Ok(false),
        _ => bail!("submit API rejected {id} (HTTP {code}): {body}"),
    }
}
