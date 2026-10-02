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
    ffi::OsString,
    fs::{self, File, TryLockError},
    io,
    path::{Path, PathBuf},
};

use amaru_kernel::{IsHeader, NetworkPoint, ORIGIN_HASH, Point};
use amaru_ledger::store::ReadStore;
use amaru_observability::info;
use amaru_ouroboros::ChainStore;
use amaru_stores::rocksdb::{ReadOnlyRocksDB, RocksDbConfig};
use anyhow::anyhow;
use same_file::Handle;
use thiserror::Error;

use super::{MithrilSyncError, store_error};
use crate::{
    chain_realign::{ClearValidity, ensure_store_consistency, realign_chain_store_to, resolve_stored_point},
    stages::build_node::{StoreOpenOperation, ledger_store_error, open_chain_store},
};

/// Store state that cannot be reconciled without rebuilding from a trusted snapshot.
#[derive(Debug, Error)]
#[error("{reason} (ledger {ledger_tip}, chain {chain_tip})")]
pub struct RebootstrapRequired {
    pub ledger_tip: Point,
    pub chain_tip: Point,
    pub reason: String,
}

/// Outcome of reconciling the adopted chain with the durable ledger tip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum StoreRecoveryOutcome {
    AlreadyConsistent {
        point: Point,
    },
    /// The adopted chain moved from `before` to `after`; the durable ledger was unchanged.
    Recovered {
        before: Point,
        after: Point,
    },
}

impl StoreRecoveryOutcome {
    /// The common ledger and adopted-chain tip after recovery.
    pub fn point(self) -> Point {
        match self {
            Self::AlreadyConsistent { point } | Self::Recovered { after: point, .. } => point,
        }
    }
}

/// Recover an interrupted store pair without network access or a snapshot cache.
///
/// Call this while the node is stopped, then await completion before opening its stores. All store
/// handles and recovery locks are released before returning. Synchronization uses the same recovery
/// implementation. [`MithrilSyncError::RebootstrapRequired`] identifies unsupported store states;
/// operational failures return [`MithrilSyncError::Store`] or [`MithrilSyncError::Startup`].
///
/// Store operations run on a Tokio blocking task. Once started, that task retains its locks and
/// finishes recovery even if the caller drops this future. Do not call this after
/// [`MithrilSyncError::WorkerShutdown`] until the process has restarted.
///
/// ```no_run
/// # async fn example() -> Result<(), amaru_node::MithrilSyncError> {
/// let outcome = amaru_node::recover_store_pair("ledger.preprod.db", "chain.preprod.db").await?;
/// # let _ = outcome;
/// # Ok(())
/// # }
/// ```
pub async fn recover_store_pair(
    ledger_dir: impl Into<PathBuf>,
    chain_dir: impl Into<PathBuf>,
) -> Result<StoreRecoveryOutcome, MithrilSyncError> {
    let ledger_dir = ledger_dir.into();
    let chain_dir = chain_dir.into();
    tokio::task::spawn_blocking(move || {
        validate_store_directories(&ledger_dir, &chain_dir)?;
        let _locks = acquire_sync_locks([ledger_dir.as_path(), chain_dir.as_path()])?;
        let chain_store = open_chain_store(&RocksDbConfig::new(chain_dir), false).map_err(MithrilSyncError::Startup)?;
        let ledger_tip = resolve_ledger_tip(&ledger_dir, &chain_store)?;
        recover_stores(&chain_store, ledger_tip)
    })
    .await
    .map_err(|source| MithrilSyncError::TaskFailed { source })?
}

/// Recover stores and return their common tip; see [`recover_store_pair`] for the detailed outcome
/// and the requirement to restart after [`MithrilSyncError::WorkerShutdown`].
pub async fn reconcile_mithril_stores(
    ledger_dir: impl Into<PathBuf>,
    chain_dir: impl Into<PathBuf>,
) -> Result<Point, MithrilSyncError> {
    recover_store_pair(ledger_dir, chain_dir).await.map(StoreRecoveryOutcome::point)
}

pub(super) fn resolve_ledger_tip(ledger_dir: &Path, chain_store: &dyn ChainStore) -> Result<Point, MithrilSyncError> {
    let ledger = ReadOnlyRocksDB::new(&RocksDbConfig::new(ledger_dir.to_path_buf()))
        .map_err(|source| MithrilSyncError::Startup(ledger_store_error(source, StoreOpenOperation::LedgerReadOnly)))?;
    let stored = NetworkPoint::from(ledger.tip().map_err(|source| store_error("read ledger tip", source))?);
    resolve_resume_point(chain_store, stored)
}

pub(super) fn resolve_resume_point(
    chain_store: &dyn ChainStore,
    stored: NetworkPoint,
) -> Result<Point, MithrilSyncError> {
    resolve_stored_point(chain_store, stored).ok_or(MithrilSyncError::ResumePointNotFound { point: stored })
}

pub(super) fn validate_store_directories(ledger_dir: &Path, chain_dir: &Path) -> Result<(), MithrilSyncError> {
    validate_store_directory(ledger_dir, "validate ledger store directory")?;
    validate_store_directory(chain_dir, "validate chain store directory")
}

fn validate_store_directory(path: &Path, operation: &'static str) -> Result<(), MithrilSyncError> {
    let metadata = fs::metadata(path).map_err(|source| store_error(operation, source))?;
    if metadata.is_dir() {
        Ok(())
    } else {
        Err(store_error(
            operation,
            io::Error::new(io::ErrorKind::NotADirectory, format!("{} is not a directory", path.display())),
        ))
    }
}

pub(super) struct SyncLock {
    file: File,
    path: PathBuf,
}

impl Drop for SyncLock {
    fn drop(&mut self) {
        if matches!(lock_points_to_path(&self.file, &self.path), Ok(true)) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

pub(super) fn acquire_sync_locks<const N: usize>(directories: [&Path; N]) -> Result<Vec<SyncLock>, MithrilSyncError> {
    let mut directories = directories
        .into_iter()
        .map(|directory| {
            fs::canonicalize(directory).map_err(|source| store_error("resolve synchronization lock path", source))
        })
        .collect::<Result<Vec<_>, _>>()?;
    directories.sort_unstable();
    directories.dedup();

    directories
        .iter()
        .map(|directory| {
            let path = sync_lock_path(directory)?;
            let file = File::create(&path).map_err(|source| store_error("create synchronization lock", source))?;
            lock_sync_file(&file, &path, directory)?;
            Ok(SyncLock { file, path })
        })
        .collect()
}

/// Detect a lock file replaced between opening it and acquiring its lock.
fn lock_points_to_path(file: &File, path: &Path) -> io::Result<bool> {
    let locked = Handle::from_file(file.try_clone()?)?;
    let current = match Handle::from_path(path) {
        Ok(current) => current,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(source) => return Err(source),
    };
    Ok(locked == current)
}

pub(super) fn sync_lock_path(directory: &Path) -> Result<PathBuf, MithrilSyncError> {
    let name = directory.file_name().ok_or_else(|| {
        store_error(
            "resolve synchronization lock path",
            io::Error::new(io::ErrorKind::InvalidInput, format!("cannot lock store root {}", directory.display())),
        )
    })?;
    let mut lock_name = OsString::from(".");
    lock_name.push(name);
    lock_name.push(".mithril-sync.lock");
    Ok(directory.with_file_name(lock_name))
}

pub(super) fn lock_sync_file(lock: &File, path: &Path, directory: &Path) -> Result<(), MithrilSyncError> {
    match lock.try_lock() {
        Ok(()) => {
            if lock_points_to_path(lock, path).map_err(|source| store_error("verify synchronization lock", source))? {
                Ok(())
            } else {
                Err(MithrilSyncError::Concurrent { path: directory.to_path_buf() })
            }
        }
        Err(TryLockError::WouldBlock) => Err(MithrilSyncError::Concurrent { path: directory.to_path_buf() }),
        Err(source) => Err(store_error("acquire synchronization lock", source)),
    }
}

pub(super) fn recover_stores(
    chain_store: &dyn ChainStore,
    ledger_tip: Point,
) -> Result<StoreRecoveryOutcome, MithrilSyncError> {
    let chain_tip = chain_store.get_best_chain_tip();
    if chain_tip == ledger_tip {
        return Ok(StoreRecoveryOutcome::AlreadyConsistent { point: ledger_tip });
    }
    let can_adopt = chain_store.load_header_with_validity(&ledger_tip.hash()).is_some_and(|(header, validity)| {
        header.point() == ledger_tip
            && validity != Some(false)
            && header.parent_hash().unwrap_or(ORIGIN_HASH) == chain_tip.hash()
            && chain_store.get_nonces(&ledger_tip.hash()).is_some()
    });
    if can_adopt {
        adopt_validated_block(chain_store, ledger_tip)
            .map_err(|source| store_error("adopt recovered ledger tip", source))?;
        info!(cli::mithril::RECOVER_CHAIN_TIP, ledger_tip, chain_tip);
    } else {
        ensure_store_consistency(chain_store, ledger_tip).map_err(|source| {
            MithrilSyncError::RebootstrapRequired(Box::new(RebootstrapRequired {
                ledger_tip,
                chain_tip,
                reason: source.to_string(),
            }))
        })?;
        realign_chain_store_to(chain_store, ledger_tip, ClearValidity::ValidOnly)
            .map_err(|source| store_error("realign chain store to ledger tip", source))?;
    }
    Ok(StoreRecoveryOutcome::Recovered { before: chain_tip, after: ledger_tip })
}

fn adopt_validated_block(chain_store: &dyn ChainStore, point: Point) -> anyhow::Result<()> {
    chain_store.set_block_valid(&point.hash(), true)?;
    chain_store.roll_forward_chain(&point)?;
    let chain_tip = chain_store.get_best_chain_tip();
    if chain_tip != point {
        anyhow::bail!("adopted chain tip {chain_tip} does not match ledger tip {point}");
    }
    Ok(())
}

/// Catch up an old replay anchor once, walking backward from the tip by the security parameter.
/// Subsequent adoptions advance it through the regular consensus stage.
pub(super) fn advance_replay_anchor(
    chain_store: &dyn ChainStore,
    tip: Point,
    security_param: u64,
) -> Result<(), MithrilSyncError> {
    let snapshot = chain_store.snapshot();
    if snapshot.get_best_chain_tip() != tip {
        return Err(store_error("find replay anchor", anyhow!("{tip} is not the adopted best-chain tip")));
    }
    let target_height = tip.block_height() - security_param;
    if target_height <= snapshot.get_anchor_point().block_height() {
        return Ok(());
    }
    let anchor =
        std::iter::successors(snapshot.load_header(&tip.hash()), |header| snapshot.load_header(&header.parent()?))
            .find(|header| header.block_height() <= target_height)
            .ok_or_else(|| {
                store_error("find replay anchor", anyhow!("missing best-chain ancestor at height {target_height}"))
            })?
            .point();
    drop(snapshot);
    chain_store.set_anchor_point(&anchor).map_err(|source| store_error("advance replay anchor", source))?;
    Ok(())
}
