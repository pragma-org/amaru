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

use std::path::Path;

use amaru_kernel::{BlockHeight, HeaderHash, IsHeader, ORIGIN_HASH, Point, cbor, size::HEADER, to_cbor};
use amaru_ouroboros_traits::{BaseReadChainStore, DiagnosticChainStore, StoreError};
use rocksdb::{DB, IteratorMode, PrefixRange, ReadOptions};
use tracing::info;

use crate::rocksdb::{
    RocksDbConfig,
    consensus::{
        RocksDBStore,
        base_read_chain_store::opcert_key,
        util::{ANCHOR_PREFIX, BEST_CHAIN_PREFIX, CHAIN_DB_VERSION, CHAIN_PREFIX, HEADER_PREFIX, open_db},
    },
};

/// The version key: __VERSION__
pub const VERSION_KEY: [u8; 11] = *b"__VERSION__";

/// List of migrations to apply, in order.
///
/// Each function at index `i` in this array corresponds to a
/// migration from version `i` to version `i + 1`.  When modifying the
/// DB schema, create migration function and add it to this array
/// bumping its length.
// NOTE: Migrations write the on-disk format of their target version
//
// Current `WriteChainStore` methods encode today's schema (for example a
// CBOR `Point` under `BEST_CHAIN_PREFIX`). Each step must therefore issue
// the RocksDB puts that were correct for the version it produces, not call
// the high-level store API of the running binary.
static MIGRATIONS: [fn(&RocksDBStore<DB>) -> Result<(), StoreError>; CHAIN_DB_VERSION as usize] =
    [migrate_to_v1, migrate_to_v2, migrate_to_v3, migrate_to_v4, migrate_to_v5, migrate_to_v6];

/// Migrate the Chain Database at the given `path` to the current `CHAIN_DB_VERSION`.
/// Returns the pair of numbers consisting in the initial version of the database and
/// the current version if migration succeeds, otherwise returns a `StoreError`.
pub fn migrate_db_path(path: &Path) -> Result<(u16, u16), StoreError> {
    let config = RocksDbConfig::new(path.to_path_buf());

    let (basedir, db) = open_db(&config)?;
    let store = RocksDBStore { db, basedir };

    migrate_db(&store)
}

/// Migrate the given `store` Chain Database to the current `CHAIN_DB_VERSION`.
/// Returns the pair of numbers consisting in the initial version of the database and
/// the current version if migration succeeds, otherwise returns a `StoreError`.
pub fn migrate_db(store: &RocksDBStore<DB>) -> Result<(u16, u16), StoreError> {
    let version = get_version(store)?;

    for n in version..CHAIN_DB_VERSION {
        info!("Migrating Chain database to version {}", n + 1);
        MIGRATIONS[n as usize](store)?
    }
    Ok((version, CHAIN_DB_VERSION))
}

/// "Migrate" DB to version 1
/// This simply records the `VERSION_KEY` into the db.
pub(crate) fn migrate_to_v1(store: &RocksDBStore<DB>) -> Result<(), StoreError> {
    set_version(store, 1)
}

/// "Migrate" DB to version 2
/// Walks the best chain backwards and re-inserts all points.
pub(crate) fn migrate_to_v2(store: &RocksDBStore<DB>) -> Result<(), StoreError> {
    let mut hash = read_legacy_hash(store, &BEST_CHAIN_PREFIX)?;
    if hash == ORIGIN_HASH {
        return Ok(());
    }

    while let Some((point, parent)) = load_stored_header_point(store, &hash) {
        store_v2_chain_entry(store, &point)?;
        match parent {
            Some(parent) => hash = parent,
            None => break,
        }
    }

    set_version(store, 2)
}

pub(crate) fn migrate_to_v3(store: &RocksDBStore<DB>) -> Result<(), StoreError> {
    // the reason is that v3 stores the block validation result, which cannot be derived from the v2 DB without
    // running the consensus algorithm and ledger validation. previously, blocks were stored before validation,
    tracing::warn!(
        "migrating chain DB to version 3 makes possibly incorrect assumption of valid best chain, better set it to the anchor hash"
    );

    let original_best_chain_hash = read_legacy_hash(store, &BEST_CHAIN_PREFIX)?;
    let anchor_hash = read_legacy_hash(store, &ANCHOR_PREFIX)?;
    // v3 stored BEST_CHAIN_PREFIX as a raw 32-byte header hash.
    store
        .db
        .put(BEST_CHAIN_PREFIX, anchor_hash.as_ref())
        .map_err(|e| StoreError::WriteError { error: e.to_string() })?;
    if anchor_hash != ORIGIN_HASH {
        store
            .db
            .put([&HEADER_PREFIX[..], &anchor_hash[..], &[0]].concat(), [1u8])
            .map_err(|e| StoreError::WriteError { error: e.to_string() })?;
    }

    tracing::info!(prev_best_chain = %original_best_chain_hash, new_best_chain = %anchor_hash, "found back best chain to revalidate");

    set_version(store, 3)
}

pub(crate) fn migrate_to_v4(store: &RocksDBStore<DB>) -> Result<(), StoreError> {
    tracing::warn!(
        "migrating chain DB to version 4: opcert sequence numbers are reconstructed from stored \
           headers only; counters from before this database was bootstrapped are unknown, which can \
           lead to incorrectly rejected headers from pools that have not produced a block since. \
           Re-bootstrapping from a snapshot is the reliable option."
    );

    for header in store.load_headers() {
        store
            .db
            .put(opcert_key(&header), to_cbor(&header.op_cert_seq()))
            .map_err(|e| StoreError::WriteError { error: e.to_string() })?;
    }
    set_version(store, 4)
}

/// Migration to version 5 is intentionally impossible.
///
/// Opcert sequence numbers must be seeded from a recent cardano-node snapshot at bootstrap.
/// Reconstructing them only from headers already in the chain store leaves most pools at an
/// implicit zero, so live opcert numbers (typically much higher) fail the Praos check.
fn migrate_to_v5(_store: &RocksDBStore<DB>) -> Result<(), StoreError> {
    Err(StoreError::OpenError {
        error: "\
chain database cannot be migrated to version 5 automatically: opcert sequence numbers for pools
are incomplete unless the chain DB was bootstrapped from a recent snapshot (values reconstructed
only from stored headers, or defaulting to zero, cause valid headers to be rejected).

Remove the existing node databases and re-bootstrap from a recent snapshot, for example:

  amaru node rm --wipe-all-dbs --network=<NETWORK>
  amaru node bootstrap --network=<NETWORK>

Then start the node with `amaru node run --network=<NETWORK>`."
            .to_string(),
    })
}

/// Rewrite best-chain tip, anchor, and per-slot chain entries from a 32-byte header hash
/// to the CBOR `Point` form (`[network_point, block_height]`).
pub(crate) fn migrate_to_v6(store: &RocksDBStore<DB>) -> Result<(), StoreError> {
    rewrite_singleton_point(store, &BEST_CHAIN_PREFIX)?;
    rewrite_singleton_point(store, &ANCHOR_PREFIX)?;
    rewrite_chain_prefix_points(store)?;
    set_version(store, 6)
}

fn rewrite_singleton_point(store: &RocksDBStore<DB>, key: &[u8]) -> Result<(), StoreError> {
    let Some(bytes) = store.db.get(key).map_err(|e| StoreError::ReadError { error: e.to_string() })? else {
        return Ok(());
    };
    if let Some(point) = point_from_legacy_hash(store, &bytes)? {
        store.db.put(key, to_cbor(&point)).map_err(|e| StoreError::WriteError { error: e.to_string() })?;
    }
    Ok(())
}

fn rewrite_chain_prefix_points(store: &RocksDBStore<DB>) -> Result<(), StoreError> {
    let mut opts = ReadOptions::default();
    opts.set_iterate_range(PrefixRange(&CHAIN_PREFIX[..]));
    let mut updates = Vec::new();
    for item in store.db.iterator_opt(IteratorMode::Start, opts) {
        let (key, value) = item.map_err(|e| StoreError::ReadError { error: e.to_string() })?;
        if let Some(point) = point_from_legacy_hash(store, &value)? {
            updates.push((key, to_cbor(&point)));
        }
    }
    store.with_batch(|batch| {
        for (key, value) in updates {
            batch.put(key, value);
        }
        Ok(())
    })
}

/// Convert a 32-byte header-hash encoding to a `Point`. Returns `Ok(None)` when `bytes` is
/// already a Point (or any other non-hash value left for the reader to reject).
fn point_from_legacy_hash(store: &RocksDBStore<DB>, bytes: &[u8]) -> Result<Option<Point>, StoreError> {
    if bytes.len() != HEADER {
        return Ok(None);
    }
    Ok(Some(point_from_hash(store, HeaderHash::from(bytes))?))
}

/// Read a pre-v6 singleton key: a 32-byte header hash, or missing (treated as origin).
fn read_legacy_hash(store: &RocksDBStore<DB>, key: &[u8]) -> Result<HeaderHash, StoreError> {
    match store.db.get(key).map_err(|e| StoreError::ReadError { error: e.to_string() })? {
        None => Ok(ORIGIN_HASH),
        Some(bytes) if bytes.len() == HEADER => Ok(HeaderHash::from(&bytes[..])),
        Some(bytes) => {
            Err(StoreError::ReadError { error: format!("expected a 32-byte header hash, got {} bytes", bytes.len()) })
        }
    }
}

fn point_from_hash(store: &RocksDBStore<DB>, hash: HeaderHash) -> Result<Point, StoreError> {
    load_stored_header_point(store, &hash).map(|(point, _)| point).ok_or_else(|| StoreError::ReadError {
        error: format!("cannot migrate header hash {hash} to Point: header not found"),
    })
}

/// Check the version stored in the `store` matches `CHAIN_DB_VERSION`.
pub fn check_db_version(store: &RocksDBStore<DB>) -> Result<(), StoreError> {
    get_version(store).and_then(|stored| {
        if stored != CHAIN_DB_VERSION {
            Err(StoreError::IncompatibleChainStoreVersions { stored, current: CHAIN_DB_VERSION })
        } else {
            Ok(())
        }
    })
}

/// Retrieve the version of the Chain DB stored in the given `store`.
/// If no version is stored, returns 0.
pub fn get_version(store: &RocksDBStore<DB>) -> Result<u16, StoreError> {
    let raw_version = store.db.get(VERSION_KEY).map_err(|e| StoreError::OpenError { error: e.to_string() })?;

    match raw_version {
        None => Ok(0),
        Some(v) => match v.as_slice() {
            [v0, v1] => Ok(((*v0 as u16) << 8) | (*v1 as u16)),
            _ => Err(StoreError::OpenError { error: format!("Invalid __VERSION__ value length: {}", v.len()) }),
        },
    }
}

/// Set the version of the Chain DB stored in the given `store` to the
/// current `CHAIN_DB_VERSION`.
pub fn set_version(store: &RocksDBStore<DB>, version: u16) -> Result<(), StoreError> {
    let bytes = version.to_be_bytes();
    store.db.put(VERSION_KEY, bytes).map_err(|e| StoreError::WriteError { error: e.to_string() })
}

/// v2 indexed the best chain by slot and stored the 32-byte header hash.
fn store_v2_chain_entry(store: &RocksDBStore<DB>, point: &Point) -> Result<(), StoreError> {
    let slot = u64::from(point.slot_or_default()).to_be_bytes();
    store
        .db
        .put([&CHAIN_PREFIX[..], &slot[..]].concat(), point.hash().as_ref())
        .map_err(|e| StoreError::WriteError { error: e.to_string() })
}

fn load_stored_header_point(store: &RocksDBStore<DB>, hash: &HeaderHash) -> Option<(Point, Option<HeaderHash>)> {
    if hash == &ORIGIN_HASH {
        return Some((Point::Origin, None));
    }
    if let Some(header) = store.load_header(hash) {
        return Some((header.point(), header.parent()));
    }

    let bytes = store.db.get([&HEADER_PREFIX[..], &hash[..]].concat()).ok()??;
    let mut decoder = cbor::Decoder::new(&bytes);
    decoder.array().ok()?;
    decoder.array().ok()?;
    let height = decoder.u64().ok()?;
    let slot = decoder.u64().ok()?;
    let parent = decoder.decode().ok()?;
    Some((Point::Specific(slot.into(), *hash, BlockHeight::from(height)), parent))
}
