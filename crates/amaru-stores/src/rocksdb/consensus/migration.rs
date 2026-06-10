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

use amaru_kernel::{IsHeader, ORIGIN_HASH, Point};
use amaru_ouroboros_traits::{BaseReadChainStore, StoreError, WriteChainStore};
use rocksdb::DB;
use tracing::info;

use crate::rocksdb::{
    RocksDbConfig,
    consensus::{
        RocksDBStore,
        util::{CHAIN_DB_VERSION, CHAIN_PREFIX, open_db},
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
    [migrate_to_v1, migrate_to_v2, migrate_to_v3];

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
fn migrate_to_v2(store: &RocksDBStore<DB>) -> Result<(), StoreError> {
    let mut hash = store.get_best_chain_hash();
    if hash == ORIGIN_HASH {
        return Ok(());
    }

    while let Some(header) = store.load_header(&hash) {
        store_chain_point(store, &header.point())?;
        match header.parent() {
            Some(parent) => hash = parent,
            None => break,
        }
    }

    set_version(store, 2)
}

#[expect(clippy::panic)]
fn migrate_to_v3(store: &RocksDBStore<DB>) -> Result<(), StoreError> {
    // the reason is that v3 stores the block validation result, which cannot be derived from the v2 DB without
    // running the consensus algorithm and ledger validation. previously, blocks were stored before validation,
    tracing::warn!(
        "migrating chain DB to version 3 makes possibly incorrect assumption of valid best chain, better set it to the anchor hash"
    );

    let original_best_chain_point = store.get_best_chain_tip().point();
    let anchor_hash = store.get_anchor_hash();
    let anchor_point = match store.load_header(&anchor_hash) {
        Some(header) => header.point(),
        None => {
            if anchor_hash == Point::Origin.hash() {
                Point::Origin
            } else {
                panic!("no header found for anchor hash {}", anchor_hash)
            }
        }
    };
    store.set_best_chain_hash(&anchor_point.hash())?;
    store.set_block_valid(&anchor_point.hash(), true)?;

    tracing::info!(prev_best_chain = %original_best_chain_point, new_best_chain = %anchor_point, "found back best chain to revalidate");

    set_version(store, 3)
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
