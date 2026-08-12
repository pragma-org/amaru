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

//! Watch addresses for UTxO / balance changes, including fork-switch undos.
//!
//! Bootstraps each address's live set from the on-disk ledger store, then keeps
//! that set in sync via adopt/undo events. Spent outputs are retained per address
//! so undos (which only carry consumed keys) can restore them.
//!
//! ```text
//! cargo run -p amaru-node --example address_watch -- \
//!   --network preprod \
//!   --address addr_test1...
//! ```

#![expect(clippy::print_stdout)]
#![expect(clippy::expect_used)]
// Address is Eq + Hash; HashMap is the natural index (project clippy bans it for libs).
#![allow(clippy::disallowed_types)]

use std::{
    collections::{HashMap, HashSet},
    env,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

use amaru_kernel::{Address, HasLovelace, MemoizedTransactionOutput, NetworkName, TransactionInput};
use amaru_ledger::store::ReadStore;
use amaru_node::{LedgerBlockEvent, LedgerObservers, NodeBuilder, UtxoDiff, default_chain_dir, default_ledger_dir};
use amaru_stores::rocksdb::{ReadOnlyRocksDB, RocksDbConfig};
use anyhow::Context;
use tracing_subscriber::EnvFilter;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with_writer(std::io::stderr)
        .init();

    let args = Args::parse(env::args().skip(1))?;
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().thread_name("amaru-address-watch").build()?;

    let index = Arc::new(Mutex::new(AddressIndex::bootstrap(&args.ledger_dir, args.addresses.iter().cloned())?));

    {
        let index = index.lock().expect("address index lock");
        for report in index.reports_for_all() {
            println!("{report}");
        }
    }

    let (notify_tx, notify_rx) = mpsc::sync_channel::<String>(64);
    let stop = Arc::new(AtomicBool::new(false));
    let stop_flag = Arc::clone(&stop);

    let index_for_obs = Arc::clone(&index);
    let mut builder = NodeBuilder::new(args.network)?
        .ledger_dir(args.ledger_dir.clone())
        .chain_dir(args.chain_dir)
        .target_upstream_peers(1)
        .listen_ephemeral_localhost()
        .observers(LedgerObservers::new().on_block(move |event| {
            let mut index = index_for_obs.lock().expect("address index lock");
            let changed = match event {
                LedgerBlockEvent::Adopted(block) => index.apply_adopt(block.utxo),
                LedgerBlockEvent::Undone(block) => index.apply_undo(block.utxo),
            };
            for id in changed {
                let _ = notify_tx.send(index.report(&id));
            }
        }));

    if !args.peer_address.is_empty() {
        builder = builder.peers(args.peer_address);
    }

    let running = builder.build_and_run(rt.handle())?;
    let running_for_term = running.clone();
    rt.spawn(async move {
        running_for_term.termination().await;
        stop_flag.store(true, Ordering::SeqCst);
    });

    while !stop.load(Ordering::SeqCst) {
        match notify_rx.recv_timeout(Duration::from_millis(200)) {
            Ok(report) => println!("{report}"),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    Ok(())
}

// -------------------------------------------------------------------------------------------------
// Address index
// -------------------------------------------------------------------------------------------------

/// UTxOs for one watched address.
///
/// - `live`: currently unspent outputs (keyed by their out-ref).
/// - `spent`: once-live outputs that were spent on the current tip path, kept so
///   undos can restore them when DiffSet only supplies the consumed input.
#[derive(Debug, Default)]
struct AddressPortfolio {
    live: HashMap<TransactionInput, Arc<MemoizedTransactionOutput>>,
    spent: HashMap<TransactionInput, Arc<MemoizedTransactionOutput>>,
}

impl AddressPortfolio {
    fn balance(&self) -> u64 {
        self.live.values().map(|output| output.lovelace()).sum()
    }

    fn report(&self, address: &Address) -> String {
        let mut lines = Vec::with_capacity(self.live.len() + 2);
        lines.push(format!("ADDRESS_CHANGE {address}"));
        let balance = self.balance();
        let ada = balance / 1_000_000;
        let lovelace = balance % 1_000_000;
        lines.push(format!("  balance: {}.{:06} lovelace  utxos: {}", ada, lovelace, self.live.len()));
        // Stable print order for humans.
        let mut entries: Vec<_> = self.live.iter().collect();
        entries.sort_by_key(|(input, _)| *input);
        for (input, output) in entries {
            lines.push(format!("    {input}: {:?}", output));
        }
        lines.join("\n")
    }
}

/// Watched addresses → their live and spent UTxO sets.
#[derive(Debug)]
struct AddressIndex {
    portfolios: HashMap<Address, AddressPortfolio>,
}

impl AddressIndex {
    /// Open the ledger store and load live UTxOs for each watched address.
    ///
    /// Any failure (missing/locked DB, iteration error) aborts the process via `?`.
    /// The scan is the stable store only; volatile tip UTxOs arrive via adopt events.
    fn bootstrap(ledger_dir: &Path, addresses: impl IntoIterator<Item = Address>) -> anyhow::Result<Self> {
        let mut portfolios = HashMap::new();
        for address in addresses {
            portfolios.entry(address).or_default();
        }

        let db = ReadOnlyRocksDB::new(&RocksDbConfig::new(ledger_dir.to_path_buf()))
            .with_context(|| format!("open ledger store at {}", ledger_dir.display()))?;

        let mut index = Self { portfolios };
        for (input, output) in db.iter_utxos().context("iterate ledger UTxOs for address bootstrap")? {
            if index.portfolios.contains_key(&output.address) {
                index.admit_live(input, Arc::new(output));
            }
        }

        let live_utxos: usize = index.portfolios.values().map(|p| p.live.len()).sum();
        tracing::info!(
            target: "address_watch",
            addresses = index.portfolios.len(),
            live_utxos,
            "bootstrapped address index from ledger store"
        );

        Ok(index)
    }

    /// Apply a tip roll-forward UTxO delta. Returns addresses whose contents changed.
    fn apply_adopt(&mut self, utxo: &UtxoDiff) -> HashSet<Address> {
        let mut changed = HashSet::new();

        for input in &utxo.consumed {
            if let Some(address) = self.spend(input) {
                changed.insert(address);
            }
        }

        for (input, output) in &utxo.produced {
            if self.portfolios.contains_key(&output.address) {
                let address = output.address.clone();
                self.admit_live(*input, Arc::clone(output));
                changed.insert(address);
            }
        }

        changed
    }

    /// Reverse a previously applied adopt (tip-first on fork switch).
    fn apply_undo(&mut self, utxo: &UtxoDiff) -> HashSet<Address> {
        let mut changed = HashSet::new();

        for input in utxo.produced.keys() {
            if let Some(address) = self.revoke_live(input) {
                changed.insert(address);
            }
        }

        for input in &utxo.consumed {
            if let Some(address) = self.restore_spent(input) {
                changed.insert(address);
            }
        }

        changed
    }

    fn report(&self, address: &Address) -> String {
        self.portfolios
            .get(address)
            .map(|p| p.report(address))
            .unwrap_or_else(|| format!("ADDRESS_CHANGE {address}\n  (unknown address)"))
    }

    fn reports_for_all(&self) -> impl Iterator<Item = String> + '_ {
        self.portfolios.iter().map(|(address, portfolio)| portfolio.report(address))
    }

    // -- mutations ------------------------------------------------------------

    fn admit_live(&mut self, input: TransactionInput, output: Arc<MemoizedTransactionOutput>) {
        let address = output.address.clone();
        let portfolio = self.portfolios.get_mut(&address).expect("admit_live only for watched addresses");
        portfolio.spent.remove(&input);
        portfolio.live.insert(input, output);
    }

    /// Move a live watched UTxO into `spent`. Returns the owning address if tracked.
    fn spend(&mut self, input: &TransactionInput) -> Option<Address> {
        for (address, portfolio) in &mut self.portfolios {
            if let Some(output) = portfolio.live.remove(input) {
                portfolio.spent.insert(*input, output);
                return Some(address.clone());
            }
        }
        None
    }

    /// Drop a live UTxO without recording it as spent (undo of a produce).
    fn revoke_live(&mut self, input: &TransactionInput) -> Option<Address> {
        for (address, portfolio) in &mut self.portfolios {
            if portfolio.live.remove(input).is_some() {
                return Some(address.clone());
            }
        }
        None
    }

    /// Re-admit a spent UTxO (undo of a consume).
    fn restore_spent(&mut self, input: &TransactionInput) -> Option<Address> {
        for (address, portfolio) in &mut self.portfolios {
            if let Some(output) = portfolio.spent.remove(input) {
                portfolio.live.insert(*input, output);
                return Some(address.clone());
            }
        }
        None
    }
}

// -------------------------------------------------------------------------------------------------
// CLI
// -------------------------------------------------------------------------------------------------

struct Args {
    network: NetworkName,
    ledger_dir: PathBuf,
    chain_dir: PathBuf,
    peer_address: Vec<String>,
    addresses: Vec<Address>,
}

impl Args {
    fn parse(mut args: impl Iterator<Item = String>) -> anyhow::Result<Self> {
        let mut network = NetworkName::Preprod;
        let mut ledger_dir = None;
        let mut chain_dir = None;
        let mut peer_address = Vec::new();
        let mut addresses = Vec::new();

        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--network" => {
                    let v = args.next().ok_or_else(|| anyhow::anyhow!("--network needs a value"))?;
                    network = v.parse().map_err(|e| anyhow::anyhow!("invalid network: {e}"))?;
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
                "--address" => {
                    let v = args.next().ok_or_else(|| anyhow::anyhow!("--address needs a value"))?;
                    let address = Address::from_bech32(&v).ok_or_else(|| anyhow::anyhow!("invalid address {v}"))?;
                    addresses.push(address);
                }
                "-h" | "--help" => {
                    eprintln!(
                        "Usage: address_watch --address <bech32> [--address ...] [--network preprod] [--ledger-dir DIR]"
                    );
                    std::process::exit(0);
                }
                other => anyhow::bail!("unknown argument: {other}"),
            }
        }

        if addresses.is_empty() {
            anyhow::bail!("at least one --address is required");
        }

        let ledger_dir = ledger_dir.unwrap_or_else(|| PathBuf::from(default_ledger_dir(network)));
        let chain_dir = chain_dir.unwrap_or_else(|| PathBuf::from(default_chain_dir(network)));

        Ok(Self { network, ledger_dir, chain_dir, peer_address, addresses })
    }
}
