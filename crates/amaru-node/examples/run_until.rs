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

//! Thin embedder: run Amaru until a target ledger epoch, then stop from outside.
//!
//! Observability is installed via [`amaru_node::Telemetry`], which honours the
//! same env knobs as the product binary (`AMARU_WITH_OPEN_TELEMETRY`,
//! `AMARU_WITH_JSON_TRACES`, `AMARU_TRACE`, `OTEL_*`).
//!
//! ```text
//! AMARU_WITH_OPEN_TELEMETRY=true cargo run -p amaru-node --example run_until -- \
//!   --network preprod --epoch 173
//! ```

use std::{
    env,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use amaru_kernel::NetworkName;
use amaru_node::{
    LedgerObservers, LogFormat, MaxExtraLedgerSnapshots, NodeBuilder, Telemetry, default_chain_dir, default_ledger_dir,
};

fn main() -> anyhow::Result<()> {
    let args = Args::parse(env::args().skip(1))?;
    let target_epoch = args.target_epoch;
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().thread_name("amaru-run-until").build()?;

    let done = Arc::new(AtomicBool::new(false));
    let reached = rt.block_on(async {
        // Install/shutdown are async so they run on this runtime (OTLP batch exporters attach to it).
        let telemetry = Telemetry::install(LogFormat::Plain).await?;

        let done_flag = Arc::clone(&done);

        let mut builder = NodeBuilder::new(args.network)?
            .ledger_dir(args.ledger_dir)
            .chain_dir(args.chain_dir)
            .target_upstream_peers(args.upstream_peers)
            .listen_ephemeral_localhost()
            .migrate_chain_db(true)
            .max_extra_ledger_snapshots(MaxExtraLedgerSnapshots::All)
            .meter(Arc::clone(&telemetry.meter))
            .observers(LedgerObservers::new().on_adopted_block(move |block| {
                if block.epoch.as_u64() >= target_epoch && !done_flag.swap(true, Ordering::SeqCst) {
                    tracing::info!(
                        target = "run_until",
                        epoch = %block.epoch,
                        point = %block.point,
                        "target epoch reached"
                    );
                }
            }));

        if !args.peer_address.is_empty() {
            builder = builder.peers(args.peer_address);
        }

        let running = builder.build_and_run(&tokio::runtime::Handle::current())?;

        loop {
            if done.load(Ordering::SeqCst) {
                running.request_abort();
                break;
            }
            tokio::select! {
                _ = running.termination() => break,
                _ = tokio::time::sleep(std::time::Duration::from_millis(200)) => {}
            }
        }
        // Wait for stages to finish so final metric samples can export.
        running.termination().await;

        telemetry.shutdown().await?;
        Ok::<bool, anyhow::Error>(done.load(Ordering::SeqCst))
    })?;

    if !reached {
        anyhow::bail!("node terminated before target epoch {target_epoch}");
    }
    Ok(())
}

struct Args {
    network: NetworkName,
    target_epoch: u64,
    ledger_dir: PathBuf,
    chain_dir: PathBuf,
    peer_address: Vec<String>,
    upstream_peers: usize,
}

impl Args {
    fn parse(mut args: impl Iterator<Item = String>) -> anyhow::Result<Self> {
        let mut network = NetworkName::Preprod;
        let mut target_epoch = None;
        let mut ledger_dir = None;
        let mut chain_dir = None;
        let mut peer_address = Vec::new();
        let mut upstream_peers = 1usize;

        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--network" => {
                    let v = args.next().ok_or_else(|| anyhow::anyhow!("--network needs a value"))?;
                    network = v.parse().map_err(|e| anyhow::anyhow!("invalid network: {e}"))?;
                }
                "--epoch" => {
                    let v = args.next().ok_or_else(|| anyhow::anyhow!("--epoch needs a value"))?;
                    target_epoch = Some(v.parse()?);
                }
                "--ledger-dir" => {
                    ledger_dir = Some(PathBuf::from(args.next().ok_or_else(|| anyhow::anyhow!("missing path"))?));
                }
                "--chain-dir" => {
                    chain_dir = Some(PathBuf::from(args.next().ok_or_else(|| anyhow::anyhow!("missing path"))?));
                }
                "--peer-address" => {
                    peer_address.push(args.next().ok_or_else(|| anyhow::anyhow!("missing peer"))?);
                }
                "--upstream-peers" => {
                    upstream_peers = args.next().ok_or_else(|| anyhow::anyhow!("missing count"))?.parse()?;
                }
                "-h" | "--help" => {
                    eprintln!(
                        "Usage: run_until --epoch <N> [--network preprod|mainnet|preview] [--ledger-dir DIR] [--chain-dir DIR]"
                    );
                    std::process::exit(0);
                }
                other => anyhow::bail!("unknown argument: {other}"),
            }
        }

        let target_epoch = target_epoch.ok_or_else(|| anyhow::anyhow!("--epoch is required"))?;
        let ledger_dir = ledger_dir.unwrap_or_else(|| PathBuf::from(default_ledger_dir(network)));
        let chain_dir = chain_dir.unwrap_or_else(|| PathBuf::from(default_chain_dir(network)));

        Ok(Self { network, target_epoch, ledger_dir, chain_dir, peer_address, upstream_peers })
    }
}
