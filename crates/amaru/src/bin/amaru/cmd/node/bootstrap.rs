// Copyright 2025 PRAGMA
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

use std::{fs, io, path::Path};

use amaru::{
    aws::S3Config,
    bootstrap::bootstrap,
    default_snapshots_dir,
    lifecycle::{Runnable, RuntimeKind},
};
use amaru_kernel::{Epoch, GlobalParameters, NetworkName, utils::path::relative_path};
use amaru_observability::{info, warn};
use clap::Parser;

#[derive(Debug, Parser)]
pub struct Args {
    /// The target bootstrap epoch; this is the epoch Amaru will start from.
    ///
    /// At least 3 past epochs must exist. When omitted, this defaults the latest available epoch
    /// from known snapshots.
    #[arg(
        long = "epoch",
        value_name = amaru::value_names::UINT,
        env = amaru::env_vars::EPOCH,
    )]
    epoch: Option<Epoch>,

    #[command(flatten)]
    network: amaru::args::Network,

    #[command(flatten, next_help_heading = "Storage options")]
    db_chain: amaru::args::DbChain,

    #[command(flatten, next_help_heading = "Storage options")]
    db_ledger: amaru::args::DbLedger,

    /// Download and import all historical blocks through the bootstrap point from Mithril.
    ///
    /// Includes Byron blocks. Requires additional download time and chain database space.
    #[cfg(feature = "mithril")]
    #[arg(long, env = "AMARU_HISTORICAL_BLOCKS", default_value_t = false)]
    historical_blocks: bool,

    #[command(flatten)]
    s3: amaru::args::S3,

    /// Override network's global parameters for custom testnets / devnets.
    ///
    /// DO NOT override for known networks (e.g. mainnet, preprod, preview, ...), as these parameters are set in stone.
    #[command(flatten, next_help_heading = "Network global parameters")]
    global_parameters: GlobalParameters,

    /// Show global network parameter overrides, for custom testnets.
    #[arg(long)]
    pub(crate) help_global_parameters: bool,
}

pub(crate) fn runnable(args: Args) -> Runnable {
    Runnable::exit_on_signal(RuntimeKind::Io, move || run(args))
}

async fn run(args: Args) -> anyhow::Result<()> {
    let network = NetworkName::from(args.network);

    let global_parameters = network.as_global_parameters().cloned().unwrap_or(args.global_parameters);

    let db_ledger = args.db_ledger.into_path_buf(network);

    let db_chain = args.db_chain.into_path_buf(network);

    info!(
        cli::node::BOOTSTRAP,
        db_chain = relative_path(&db_chain)?.display().to_string(),
        db_ledger = relative_path(&db_ledger)?.display().to_string(),
        network,
        epoch = @args.epoch.map(|e| e.to_string()),
    );

    let ledger_db_populated = is_populated(&db_ledger)?;
    let chain_db_populated = is_populated(&db_chain)?;

    if ledger_db_populated || chain_db_populated {
        let mut messages = Vec::new();

        if ledger_db_populated {
            let dir = relative_path(&db_ledger)?.display().to_string();
            let hint = "ledger directory already exists: use another location or remove it manually";
            warn!(cli::db_ledger::EXIST, dir, hint);
            messages.push(format!("{hint} ({dir})"));
        }

        if chain_db_populated {
            let dir = relative_path(&db_chain)?.display().to_string();
            let hint = "chain directory already exists: use another location or remove it manually";
            warn!(cli::db_chain::EXIST, dir, hint);
            messages.push(format!("{hint} ({dir})"));
        }

        anyhow::bail!("{}", messages.join("; "));
    }

    let report = bootstrap(
        network,
        &global_parameters,
        db_ledger,
        db_chain.clone(),
        default_snapshots_dir(network).into(),
        args.epoch,
        S3Config {
            bucket: args.s3.bucket,
            endpoint: args.s3.endpoint,
            region: args.s3.region,
            public_url: args.s3.public_url,
        },
        amaru_bootstrap::BootstrapCancellation::new(),
    )
    .await?;

    #[cfg(feature = "mithril")]
    if args.historical_blocks {
        historical::bootstrap(network, db_chain, report.point).await?;
    }
    #[cfg(not(feature = "mithril"))]
    let _ = report;

    Ok(())
}

#[cfg(feature = "mithril")]
mod historical {
    use std::{
        fs,
        path::{Path, PathBuf},
        sync::{
            Arc,
            atomic::{AtomicU64, Ordering},
        },
    };

    use amaru_bootstrap::{BootstrapObserver, BootstrapProgress};
    use amaru_kernel::{HeaderHash, NetworkName, NetworkPoint, Point, RawBlock};
    use amaru_mithril::{
        MithrilDownloadObserver, MithrilDownloadProgress, download_from_mithril_for_range_with_observer,
        iter_immutable_raw_blocks,
    };
    use amaru_progress_bar::{ProgressBar, TerminalProgressBar};
    use amaru_stores::rocksdb::{RocksDbConfig, consensus::RocksDBStore};

    struct Progress {
        bar: TerminalProgressBar,
        completed: AtomicU64,
    }

    impl Progress {
        fn new(template: &str) -> Self {
            Self { bar: TerminalProgressBar::new(0_u64, template), completed: AtomicU64::new(0) }
        }
    }

    impl Drop for Progress {
        fn drop(&mut self) {
            self.bar.clear_shared();
        }
    }

    impl MithrilDownloadObserver for Progress {
        fn on_progress(&self, progress: MithrilDownloadProgress) {
            if let MithrilDownloadProgress::Downloaded { completed_files, .. } = progress {
                let previous = self.completed.swap(completed_files, Ordering::Relaxed);
                self.bar.tick(completed_files.saturating_sub(previous) as usize);
            }
        }
    }

    impl BootstrapObserver for Progress {
        fn on_progress(&self, _: BootstrapProgress) {}

        fn on_historical_block(&self, _: HeaderHash, _: &RawBlock) {
            self.bar.increment();
        }
    }

    pub(super) async fn bootstrap(
        network: NetworkName,
        chain_dir: PathBuf,
        through: NetworkPoint,
    ) -> anyhow::Result<()> {
        let snapshots_dir = Path::new("mithril-snapshots").join(network.to_string().to_lowercase());
        fs::create_dir_all(&snapshots_dir)?;
        let observer = Arc::new(Progress::new(
            "{spinner:.green} {elapsed_precise} Downloading and verifying history ({human_pos} files)",
        ));
        let download = download_from_mithril_for_range_with_observer(
            network,
            snapshots_dir,
            Point::Origin,
            Some(through.slot_or_default()),
            observer.clone(),
        )
        .await?;
        drop(observer);
        tokio::task::spawn_blocking(move || import(network, &chain_dir, &download.immutable_dir, through)).await?
    }

    pub(super) fn import(
        network: NetworkName,
        chain_dir: &Path,
        immutable_dir: &Path,
        through: NetworkPoint,
    ) -> anyhow::Result<()> {
        let store = RocksDBStore::open(&RocksDbConfig::new(chain_dir.to_path_buf()))?;
        let observer = Progress::new("{spinner:.green} Importing historical blocks {human_pos} ({per_sec})");
        amaru_bootstrap::bootstrap::import_historical_blocks(
            &store,
            iter_immutable_raw_blocks(immutable_dir, network)?,
            through,
            &observer,
        )
    }
}

/// Whether `dir` holds anything. An empty directory is no more a database than a missing one, and
/// the build script keeps empty ledger directories around for cargo to watch.
///
/// A directory that cannot be read is not reported as empty: this guards existing databases, so it
/// must not let a bootstrap through on a failed inspection.
fn is_populated(dir: &Path) -> Result<bool, io::Error> {
    match fs::read_dir(dir) {
        Ok(mut entries) => Ok(entries.next().is_some()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(io::Error::new(err.kind(), format!("{}: {err}", dir.display()))),
    }
}

#[cfg(all(test, feature = "mithril"))]
mod tests {
    use amaru_kernel::{
        EraHistory, HeaderHash, IsHeader, NetworkName, NetworkPoint, Point, cardano::network_block::EncodedTestBlock,
        make_header,
    };
    use amaru_ouroboros::{BaseReadChainStore, DiagnosticChainStore, ReadChainStore, WriteChainStore};
    use amaru_stores::rocksdb::{
        RocksDbConfig,
        consensus::{RocksDBStore, get_version, set_version},
    };
    use tempfile::TempDir;

    use super::{fs, historical::import as import_historical_blocks};

    #[test]
    fn imports_opaque_history_in_batches_through_the_bootstrap_block() {
        let dir = TempDir::new().unwrap();
        let chain_dir = dir.path().join("chain");
        let config = RocksDbConfig::new(chain_dir.clone());
        let opaque_hash = HeaderHash::from([42; 32]);
        let tip = EncodedTestBlock::from_seed(&make_header(2, 2, Some(opaque_hash)), &EraHistory::default());
        let later = EncodedTestBlock::from_seed(&make_header(3, 3, Some(tip.header.hash())), &EraHistory::default());
        let store = RocksDBStore::create(config.clone()).unwrap();
        store.store_block(&tip.header.hash(), &tip.raw).unwrap();
        let tip_metadata = Some((tip.header.point().into(), Some(opaque_hash)));
        assert_eq!(store.load_block_metadata(&tip.header.hash()).unwrap(), tip_metadata);
        store.store_header(&tip.header).unwrap();
        store.store_header(&later.header).unwrap();
        assert_eq!(store.load_block_metadata(&later.header.hash()).unwrap(), None);
        set_version(&store, 6).unwrap();
        drop(store);
        let store = RocksDBStore::open_and_migrate(&config).unwrap();
        assert_eq!(get_version(&store).unwrap(), 7);
        assert_eq!(store.snapshot().load_block_metadata(&tip.header.hash()).unwrap(), tip_metadata);
        drop(store);
        let mut opaque = amaru_kernel::to_cbor(&(0u8, ((0u32, HeaderHash::from([0; 32])),)));
        opaque.resize(8 * 1024 * 1024, 0xff);
        let blocks = [
            (NetworkPoint::Specific(0.into(), opaque_hash), opaque),
            (tip.header.point().into(), tip.raw.to_vec()),
            (later.header.point().into(), later.raw.to_vec()),
        ];
        let mut chunk = Vec::new();
        let mut secondary = Vec::new();
        for (point, bytes) in &blocks {
            let mut entry = [0; 56];
            entry[..8].copy_from_slice(&(chunk.len() as u64).to_be_bytes());
            entry[16..48].copy_from_slice(point.hash().as_ref());
            entry[48..56].copy_from_slice(&point.slot_or_default().as_u64().to_be_bytes());
            secondary.extend_from_slice(&entry);
            chunk.extend_from_slice(bytes);
        }
        fs::write(dir.path().join("00000.chunk"), chunk).unwrap();
        fs::write(dir.path().join("00000.secondary"), secondary).unwrap();
        fs::write(dir.path().join("00000.primary"), []).unwrap();

        import_historical_blocks(NetworkName::Mainnet, &chain_dir, dir.path(), tip.header.point().to_network_point())
            .unwrap();

        let store = RocksDBStore::open(&config).unwrap();
        assert!(store.has_block(&opaque_hash).unwrap());
        assert_eq!(store.load_block(&opaque_hash).unwrap().unwrap().as_ref(), blocks[0].1);
        assert_eq!(store.snapshot().load_block_metadata(&opaque_hash).unwrap(), Some((blocks[0].0, None)));
        assert_eq!(
            store.snapshot().load_block_metadata(&tip.header.hash()).unwrap(),
            Some((blocks[1].0, Some(opaque_hash)))
        );
        assert_eq!(store.load_block(&tip.header.hash()).unwrap(), Some(tip.raw));
        assert!(!store.has_block(&later.header.hash()).unwrap());
        assert_eq!(store.get_best_chain_tip(), Point::Origin);
        assert_eq!(store.get_anchor_point(), Point::Origin);
        assert_eq!(store.load_blocks().count(), 2);
        store.remove_block(&opaque_hash).unwrap();
        assert_eq!(store.load_block_metadata(&opaque_hash).unwrap(), None);
        assert_eq!(store.load_block(&opaque_hash).unwrap(), None);
        drop(store);

        let missing = Point::Specific(9.into(), HeaderHash::from([9; 32]), 9.into()).to_network_point();
        let error = import_historical_blocks(NetworkName::Mainnet, &chain_dir, dir.path(), missing).unwrap_err();
        assert!(error.to_string().contains("was not found"));
    }
}
