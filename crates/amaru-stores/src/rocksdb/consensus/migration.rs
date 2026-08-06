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

use amaru_kernel::{HeaderHash, IsHeader, ORIGIN_HASH, Point, cbor, to_cbor};
use amaru_ouroboros_traits::{BaseReadChainStore, DiagnosticChainStore, StoreError, WriteChainStore};
use rocksdb::DB;
use tracing::info;

use crate::rocksdb::{
    RocksDbConfig,
    consensus::{
        RocksDBStore,
        base_read_chain_store::opcert_key,
        util::{CHAIN_DB_VERSION, CHAIN_PREFIX, HEADER_PREFIX, open_db},
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
static MIGRATIONS: [fn(&RocksDBStore<DB>) -> Result<(), StoreError>; CHAIN_DB_VERSION as usize] =
    [migrate_to_v1, migrate_to_v2, migrate_to_v3, migrate_to_v4, migrate_to_v5];

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
    let mut hash = store.get_best_chain_hash();
    if hash == ORIGIN_HASH {
        return Ok(());
    }

    while let Some((point, parent)) = load_stored_header_point(store, &hash) {
        store_chain_point(store, &point)?;
        match parent {
            Some(parent) => hash = parent,
            None => break,
        }
    }

    set_version(store, 2)
}

#[expect(clippy::panic)]
pub(crate) fn migrate_to_v3(store: &RocksDBStore<DB>) -> Result<(), StoreError> {
    // the reason is that v3 stores the block validation result, which cannot be derived from the v2 DB without
    // running the consensus algorithm and ledger validation. previously, blocks were stored before validation,
    tracing::warn!(
        "migrating chain DB to version 3 makes possibly incorrect assumption of valid best chain, better set it to the anchor hash"
    );

    let original_best_chain_hash = store.get_best_chain_hash();
    let original_best_chain_point = load_stored_header_point(store, &original_best_chain_hash)
        .map(|(point, _)| point)
        .ok_or_else(|| StoreError::ReadError {
            error: format!("best chain tip {original_best_chain_hash} was not found during migration"),
        })?;
    let anchor_hash = store.get_anchor_hash();
    let anchor_point = load_stored_header_point(store, &anchor_hash)
        .map(|(point, _)| point)
        .unwrap_or_else(|| panic!("no header found for anchor hash {}", anchor_hash));
    store.set_best_chain_hash(&anchor_point.hash())?;
    store.set_block_valid(&anchor_point.hash(), true)?;

    tracing::info!(prev_best_chain = %original_best_chain_point, new_best_chain = %anchor_point, "found back best chain to revalidate");

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

fn store_chain_point(store: &RocksDBStore<DB>, point: &Point) -> Result<(), StoreError> {
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
    decoder.skip().ok()?;
    let slot = decoder.u64().ok()?;
    let parent = decoder.decode().ok()?;
    Some((Point::Specific(slot.into(), *hash), parent))
}
