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

//! Discover the `run_until` target epoch from Amaru's published bootstrap index.
//!
//! Fixture helpers for recorded-chain world tests (`real_data`; EDR-011
//! "World tests: generated vs recorded chains").
//!
//! Bootstrap lists `<network>/index.json` (see `amaru-bootstrap::AnonymousS3Client::list_snapshots`)
//! and maps each `<slot>.<hash>` point through [`EraHistory::slot_to_epoch_unchecked_horizon`].
//! The latest snapshot epoch is that maximum. `run_until` stops at the first block of
//! `latest + 2` so the fragment is the following full epoch.

use std::{
    fs::{self, File, TryLockError},
    io,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use amaru_bootstrap::{DEFAULT_PUBLIC_URL, S3Config, bootstrap, default_snapshots_dir};
use amaru_kernel::{Epoch, EraHistory, Header, HeaderHash, IsHeader, NetworkName, Slot};
use amaru_ledger::LedgerObservers;
use amaru_metrics::Meter;
use amaru_ouroboros::BaseReadChainStore;
use amaru_stores::rocksdb::{RocksDbConfig, consensus::RocksDBStore};
use serde::Deserialize;
use tokio::runtime::Builder;

use crate::{LogFormat, MaxExtraLedgerSnapshots, NodeBuilder, Telemetry};

const RUN_UNTIL_UPSTREAM_PEERS: usize = 10;

/// `<slot>.<hash>` point as published in `<network>/index.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotPoint {
    pub point: String,
    pub slot: Slot,
    pub epoch: Epoch,
}

/// Result of mapping a bootstrap index through era history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapIndex {
    pub latest: SnapshotPoint,
    pub target_epoch: Epoch,
}

/// Committed metadata written next to the on-disk fixture stores.
#[derive(Debug, Clone, Deserialize)]
pub struct FragmentMeta {
    pub network: String,
    pub index_source: String,
    pub latest_snapshot_point: String,
    pub latest_snapshot_epoch: u64,
    pub target_epoch: u64,
    /// Last header after the snapshot that has a stored body. Display form of [`Point`].
    pub fragment_head: String,
}

/// Parse `<slot>.<hash>` the same way `amaru-bootstrap` does.
pub fn parse_slot_from_point(point: &str) -> anyhow::Result<u64> {
    point
        .split('.')
        .next()
        .and_then(|s| s.parse::<u64>().ok())
        .ok_or_else(|| anyhow::anyhow!("invalid snapshot point format: {point}"))
}

/// Map published index points to epochs. Latest snapshot sits at the end of its epoch;
/// `run_until` stops at the first block of `latest + 2` so the fragment is the whole
/// next epoch (~`epoch_size` slots), not the few blocks until that epoch begins.
pub fn bootstrap_index_from_points(points: &[String], era_history: &EraHistory) -> anyhow::Result<BootstrapIndex> {
    let mut snapshots = Vec::with_capacity(points.len());
    for point in points {
        let slot = Slot::from(parse_slot_from_point(point)?);
        let epoch = era_history.slot_to_epoch_unchecked_horizon(slot)?;
        snapshots.push(SnapshotPoint { point: point.clone(), slot, epoch });
    }
    let latest = snapshots
        .into_iter()
        .max_by_key(|s| s.epoch)
        .ok_or_else(|| anyhow::anyhow!("bootstrap index listed no snapshots"))?;
    Ok(BootstrapIndex { target_epoch: latest.epoch + 2, latest })
}

pub fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/world-preprod-fragment")
}

/// `snapshots/<network>/` at the workspace root (`./snapshots`), not the crate CWD.
///
/// `cargo test -p amaru-node` runs with cwd `crates/amaru-node`, so a relative
/// [`default_snapshots_dir`] would download a second copy of the archives.
fn workspace_snapshots_dir(network: NetworkName) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..").join(default_snapshots_dir(network))
}

pub fn load_committed_index(root: &Path) -> anyhow::Result<Vec<String>> {
    let bytes = fs::read(root.join("index.json"))?;
    Ok(serde_json::from_slice(&bytes)?)
}

pub fn load_committed_meta(root: &Path) -> anyhow::Result<FragmentMeta> {
    let bytes = fs::read(root.join("meta.json"))?;
    Ok(serde_json::from_slice(&bytes)?)
}

pub fn stores_ready(root: &Path) -> bool {
    bootstrap_ready(root) && primed_ready(root)
}

fn bootstrap_ready(root: &Path) -> bool {
    dir_is_populated(&root.join("bootstrap/chain")) && dir_is_populated(&root.join("bootstrap/ledger"))
}

fn primed_ready(root: &Path) -> bool {
    dir_is_populated(&root.join("primed/chain")) && dir_is_populated(&root.join("primed/ledger"))
}

/// Produce `bootstrap/` and `primed/` under `root` when they are missing, or
/// rebuild `primed/` when it only covers the tail after the snapshot rather
/// than the following epoch.
///
/// Same steps as the fixture README: public-CDN bootstrap, copy, then live
/// `run_until` of `meta.target_epoch` from embedded big-ledger peers. Coverage is checked
/// under the populate lock so a concurrent rebuild cannot make the other test
/// open a half-written store.
pub fn ensure_fragment_stores(root: &Path) -> anyhow::Result<()> {
    let _lock = acquire_populate_lock(root)?;
    if fragment_stores_usable(root)? {
        return Ok(());
    }
    let meta = load_committed_meta(root)?;
    let rt = Builder::new_multi_thread().enable_all().thread_name("world-fragment-populate").build()?;
    rt.block_on(populate_fragment_stores(root, &meta))
}

fn fragment_stores_usable(root: &Path) -> anyhow::Result<bool> {
    if !stores_ready(root) {
        return Ok(false);
    }
    let meta = load_committed_meta(root)?;
    primed_covers_next_epoch(root, &meta)
}

/// Exclusive lock on `root/.populate.lock` so parallel `--ignored` tests share one populate.
///
/// [`File::lock`] is advisory whole-file locking: `flock(LOCK_EX)` on Unix, `LockFileEx` on
/// Windows. The OS drops it when this handle is closed (including process crash). The file
/// is left in place; presence is not the lock.
fn acquire_populate_lock(root: &Path) -> anyhow::Result<File> {
    fs::create_dir_all(root)?;
    let path = root.join(".populate.lock");
    let file = File::create(&path)?;
    match file.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => {
            eprintln!("waiting for another test to finish producing fragment stores ({})", path.display());
            file.lock()?;
        }
        Err(e) => return Err(e.into()),
    }
    Ok(file)
}

async fn populate_fragment_stores(root: &Path, meta: &FragmentMeta) -> anyhow::Result<()> {
    let _telemetry = Telemetry::install_local(LogFormat::Ansi)?;
    tracing::info!(
        target = "world_fragment",
        path = %root.display(),
        target_epoch = meta.target_epoch,
        "producing bootstrap + primed stores"
    );

    if !bootstrap_ready(root) {
        tracing::info!(target = "world_fragment", "bootstrap latest published preprod snapshot into bootstrap/");
        let chain = root.join("bootstrap/chain");
        let ledger = root.join("bootstrap/ledger");
        remove_if_exists(&chain)?;
        remove_if_exists(&ledger)?;
        fs::create_dir_all(&chain)?;
        fs::create_dir_all(&ledger)?;
        let network = NetworkName::Preprod;
        let global =
            network.as_global_parameters().cloned().ok_or_else(|| anyhow::anyhow!("preprod global parameters"))?;
        let snapshots_dir = workspace_snapshots_dir(network);
        tracing::info!(
            target = "world_fragment",
            snapshots_dir = %snapshots_dir.display(),
            "reusing workspace snapshot cache"
        );
        bootstrap(network, &global, ledger, chain, snapshots_dir, None, S3Config::default())
            .await
            .map_err(|e| anyhow::anyhow!("bootstrap: {e}"))?;
    } else {
        tracing::info!(target = "world_fragment", "bootstrap/ already populated; skipping snapshot download");
    }

    if primed_ready(root) && !primed_covers_next_epoch(root, meta)? {
        tracing::info!(
            target = "world_fragment",
            "primed fragment is only a tail of an epoch; removing it to sync a full epoch"
        );
        remove_if_exists(&root.join("primed"))?;
    }

    if !primed_ready(root) {
        tracing::info!(target = "world_fragment", "copy bootstrap → primed");
        let primed = root.join("primed");
        remove_if_exists(&primed)?;
        copy_dir(&root.join("bootstrap"), &primed)?;
        tracing::info!(
            target = "world_fragment",
            epoch = meta.target_epoch,
            target_upstream_peers = RUN_UNTIL_UPSTREAM_PEERS,
            "run_until target epoch from embedded big-ledger peers"
        );
        run_until_target_epoch(&primed, meta, Arc::new(Meter::default())).await?;
    } else {
        tracing::info!(target = "world_fragment", "primed/ already populated; skipping run_until");
    }

    if !stores_ready(root) {
        anyhow::bail!("fragment stores still missing after populate under {}", root.display());
    }
    if !primed_covers_next_epoch(root, meta)? {
        anyhow::bail!("primed fragment is still shorter than one epoch after populate under {}", root.display());
    }
    tracing::info!(target = "world_fragment", "stores ready");
    Ok(())
}

fn remove_if_exists(path: &Path) -> io::Result<()> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

async fn run_until_target_epoch(primed: &Path, meta: &FragmentMeta, meter: Arc<Meter>) -> anyhow::Result<()> {
    let target_epoch = meta.target_epoch;
    let done = Arc::new(AtomicBool::new(false));
    let done_flag = Arc::clone(&done);
    let running = NodeBuilder::new(NetworkName::Preprod)?
        .ledger_dir(primed.join("ledger"))
        .chain_dir(primed.join("chain"))
        .no_default_peer()
        .target_upstream_peers(RUN_UNTIL_UPSTREAM_PEERS)
        .listen_ephemeral_localhost()
        .migrate_chain_db(true)
        .max_extra_ledger_snapshots(MaxExtraLedgerSnapshots::All)
        .meter(meter)
        .observers(LedgerObservers::new().on_adopted_block(move |block| {
            if block.epoch.as_u64() >= target_epoch && !done_flag.swap(true, Ordering::SeqCst) {
                tracing::info!(
                    target = "run_until",
                    epoch = %block.epoch,
                    point = %block.point,
                    "target epoch reached"
                );
            }
        }))
        .build_and_run(&tokio::runtime::Handle::current())?;

    loop {
        if done.load(Ordering::SeqCst) {
            running.request_abort();
            break;
        }
        tokio::select! {
            _ = running.termination() => break,
            _ = tokio::time::sleep(Duration::from_millis(200)) => {}
        }
    }
    running.termination().await;
    if !done.load(Ordering::SeqCst) {
        anyhow::bail!("node terminated before target epoch {target_epoch}");
    }
    Ok(())
}

/// True when the primed chain spans most of an epoch after the snapshot (not a ~epoch-boundary tail).
fn primed_covers_next_epoch(root: &Path, meta: &FragmentMeta) -> anyhow::Result<bool> {
    let store = open_chain_store(&root.join("primed/chain"))?;
    let snapshot = header_hash_from_snapshot_point(&meta.latest_snapshot_point)?;
    let Ok(fragment) = linear_fragment_with_bodies(&store, snapshot) else {
        return Ok(false);
    };
    let Some(head) = fragment.last() else {
        return Ok(false);
    };
    let snapshot_slot = parse_slot_from_point(&meta.latest_snapshot_point)?;
    Ok(covers_following_epoch(head.slot().as_u64(), snapshot_slot))
}

/// Conway/Shelley+ preprod epoch length in slots.
pub(super) const PREPROD_EPOCH_SLOTS: u64 = 432_000;

pub(super) fn covers_following_epoch(head_slot: u64, snapshot_slot: u64) -> bool {
    head_slot.saturating_sub(snapshot_slot) >= PREPROD_EPOCH_SLOTS * 8 / 10
}

fn dir_is_populated(path: &Path) -> bool {
    fs::read_dir(path).ok().is_some_and(|mut entries| entries.next().is_some())
}

pub fn copy_dir(src: &Path, dst: &Path) -> io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let dest = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &dest)?;
        } else {
            match fs::copy(entry.path(), &dest) {
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e),
            }
        }
    }
    Ok(())
}

pub fn open_chain_store(chain_dir: &Path) -> anyhow::Result<RocksDBStore> {
    Ok(RocksDBStore::open_for_readonly(&RocksDbConfig::new(chain_dir.to_path_buf()))?)
}

/// Hash part of a `<slot>.<hash>` bootstrap index point.
pub fn header_hash_from_snapshot_point(point: &str) -> anyhow::Result<HeaderHash> {
    let hash = point
        .split_once('.')
        .map(|(_, hash)| hash)
        .ok_or_else(|| anyhow::anyhow!("invalid snapshot point format: {point}"))?;
    hash.parse().map_err(|e| anyhow::anyhow!("invalid snapshot hash in {point}: {e}"))
}

/// Headers on the parent chain from `after` (exclusive) through `head` (inclusive).
///
/// For a linear fragment this ends at the fragment HEAD, not the first header after the intersection.
pub fn linear_fragment_to_head(
    store: &dyn BaseReadChainStore,
    after: HeaderHash,
    head: Header,
) -> anyhow::Result<Vec<Header>> {
    let mut headers = vec![head.clone()];
    let mut current = head;
    loop {
        let Some(parent) = current.parent() else {
            anyhow::bail!("reached origin before snapshot {after}");
        };
        if parent == after {
            headers.reverse();
            return Ok(headers);
        }
        current = store.load_header(&parent).ok_or_else(|| anyhow::anyhow!("missing parent {parent}"))?;
        headers.push(current.clone());
    }
}

/// Best-chain headers after `after` that already have bodies.
///
/// `run_until` can leave headers ahead of the last stored block. The disseminable HEAD is the
/// last header in this list, not the first header after the snapshot.
pub fn linear_fragment_with_bodies(store: &dyn BaseReadChainStore, after: HeaderHash) -> anyhow::Result<Vec<Header>> {
    let Some(mut cursor) = store.load_point(&after) else {
        anyhow::bail!("snapshot header {after} is not in the store");
    };
    let mut headers = Vec::new();
    while let Some(next) = store.next_best_chain(&cursor) {
        if !store.has_block(&next.hash())? {
            break;
        }
        let header = store.load_header(&next.hash()).ok_or_else(|| anyhow::anyhow!("missing header for {next}"))?;
        headers.push(header);
        cursor = next;
    }
    if headers.is_empty() {
        anyhow::bail!("primed store has no fragment bodies after {after}");
    }
    Ok(headers)
}

#[cfg(test)]
mod tests {
    use amaru_kernel::PREPROD_ERA_HISTORY;

    use super::*;

    #[test]
    fn test_target_epoch_is_discovered_from_bootstrap_index() {
        let root = fixture_root();
        let points = load_committed_index(&root).expect("committed preprod/index.json");
        let discovered =
            bootstrap_index_from_points(&points, &PREPROD_ERA_HISTORY).expect("map index through era history");
        let meta = load_committed_meta(&root).expect("committed meta.json");

        assert_eq!(meta.network, "preprod");
        assert_eq!(meta.index_source, format!("{DEFAULT_PUBLIC_URL}/preprod/index.json"));
        assert_eq!(discovered.latest.point, meta.latest_snapshot_point);
        assert_eq!(discovered.latest.epoch.as_u64(), meta.latest_snapshot_epoch);
        assert_eq!(discovered.target_epoch.as_u64(), meta.target_epoch);
        assert_eq!(discovered.target_epoch, discovered.latest.epoch + 2);
        assert!(!meta.fragment_head.is_empty(), "meta.fragment_head records a previous production HEAD");
    }

    #[test]
    fn test_workspace_snapshots_dir_is_repo_cache() {
        let dir = workspace_snapshots_dir(NetworkName::Preprod);
        assert!(dir.ends_with(Path::new("snapshots").join("preprod")));
        assert_ne!(dir, PathBuf::from(default_snapshots_dir(NetworkName::Preprod)));
    }

    #[test]
    fn test_covers_following_epoch_rejects_boundary_tail() {
        // meta.json's previous fragment_head is 174 slots after the snapshot.
        assert!(!covers_following_epoch(130_982_572, 130_982_398));
        assert!(covers_following_epoch(130_982_398 + PREPROD_EPOCH_SLOTS * 8 / 10, 130_982_398));
    }

    #[test]
    fn test_parse_slot_from_bootstrap_point() {
        let point = "130982398.6b78e3cbc65e4cc9ca036c03ab125697b9a31954f55219bf7ad5397d63286c43";
        assert_eq!(parse_slot_from_point(point).unwrap(), 130_982_398);
        assert_eq!(
            header_hash_from_snapshot_point(point).unwrap().to_string(),
            "6b78e3cbc65e4cc9ca036c03ab125697b9a31954f55219bf7ad5397d63286c43"
        );
    }

    #[test]
    #[ignore = "first run downloads a preprod snapshot and syncs one epoch from the network"]
    fn test_primed_store_fragment_head_is_last_header_with_body() {
        let root = fixture_root();
        ensure_fragment_stores(&root).expect("produce preprod fragment stores");
        let meta = load_committed_meta(&root).expect("meta.json");
        let tmp = tempfile::tempdir().expect("copy primed chain");
        copy_dir(&root.join("primed/chain"), &tmp.path().join("chain")).expect("copy primed chain");
        let store = open_chain_store(&tmp.path().join("chain")).expect("open primed chain");
        let snapshot = header_hash_from_snapshot_point(&meta.latest_snapshot_point).expect("snapshot hash");
        let fragment = linear_fragment_with_bodies(&store, snapshot).expect("fragment with bodies");
        let head = fragment.last().expect("HEAD");
        let snapshot_slot = parse_slot_from_point(&meta.latest_snapshot_point).expect("snapshot slot");
        assert!(
            covers_following_epoch(head.slot().as_u64(), snapshot_slot),
            "fragment should cover most of an epoch; got {} headers ending at {}",
            fragment.len(),
            head.point()
        );
        let walked = linear_fragment_to_head(&store, snapshot, head.clone()).expect("parent walk");
        assert_eq!(walked.last().map(|h| h.point()), Some(head.point()));
        assert!(fragment.len() >= 2, "fragment must be more than a single header");
        assert_ne!(
            format!("{}", fragment[0].point()),
            format!("{}", head.point()),
            "HEAD must not be the first header after the snapshot"
        );
    }
}
