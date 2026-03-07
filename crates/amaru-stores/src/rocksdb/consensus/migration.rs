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

use amaru_kernel::{BlockHeader, Hash, HeaderHash, IsHeader, ORIGIN_HASH, Point, cardano::hash, from_cbor};
use amaru_ouroboros_traits::StoreError;
use rocksdb::OptimisticTransactionDB;
use tracing::info;

use crate::rocksdb::{
    RocksDbConfig,
    consensus::{
        store_chain_point,
        util::{ANCHOR_PREFIX, BEST_CHAIN_PREFIX, CHAIN_DB_VERSION, HEADER_PREFIX, open_db},
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
static MIGRATIONS: [fn(&OptimisticTransactionDB) -> Result<(), StoreError>; CHAIN_DB_VERSION as usize] =
    [migrate_to_v1, migrate_to_v2, migrate_to_v3];

/// Migrate the Chain Database at the given `path` to the current `CHAIN_DB_VERSION`.
/// Returns the pair of numbers consisting in the initial version of the database and
/// the current version if migration succeeds, otherwise returns a `StoreError`.
pub fn migrate_db_path(path: &Path) -> Result<(u16, u16), StoreError> {
    let config = RocksDbConfig::new(path.to_path_buf());

    let (_, db) = open_db(&config)?;

    migrate_db(&db)
}

/// Migrate the given `db` Chain Database to the current `CHAIN_DB_VERSION`.
/// Returns the pair of numbers consisting in the initial version of the database and
/// the current version if migration succeeds, otherwise returns a `StoreError`.
pub fn migrate_db(db: &OptimisticTransactionDB) -> Result<(u16, u16), StoreError> {
    let version = get_version(db)?;

    for n in version..CHAIN_DB_VERSION {
        info!("Migrating Chain database to version {}", n + 1);
        MIGRATIONS[n as usize](db)?
    }
    Ok((version, CHAIN_DB_VERSION))
}

/// "Migrate" DB to version 1
/// This simply records the `VERSION_KEY` into the db.
pub(crate) fn migrate_to_v1(db: &OptimisticTransactionDB) -> Result<(), StoreError> {
    set_version(db, 1)?;
    Ok(())
}

/// "Migrate" DB to version 2
/// Walks the best chain backwards and re-inserts all points.
fn migrate_to_v2(db: &OptimisticTransactionDB) -> Result<(), StoreError> {
    let mut hash = match get_best_chain_hash(db) {
        Some(hash) => hash,
        // the DB is empty, nothing to do
        None => return Ok(()),
    };

    while let Some(header) = load_header(db, &hash) {
        store_chain_point(db, &header.point())?;
        match header.parent() {
            Some(parent) => hash = parent,
            None => break,
        }
    }

    set_version(db, 2)?;
    Ok(())
}

fn migrate_to_v3(db: &OptimisticTransactionDB) -> Result<(), StoreError> {
    // the reason is that v3 stores the block validation result, which cannot be derived from the v2 DB without
    // running the consensus algorithm and ledger validation. previously, blocks were stored before validation,
    tracing::warn!(
        "migrating chain DB to version 3 makes possibly incorrect assumption of valid anchor, better start fresh"
    );

    let (anchor_hash, anchor_point) = if let Some(anchor_hash) = get_anchor_hash(db)
        && let Some(anchor_header) = load_header(db, &anchor_hash)
    {
        (anchor_hash, anchor_header.point())
    } else {
        (ORIGIN_HASH, Point::Origin)
    };
    let best_chain_point = if let Some(best_chain_hash) = get_best_chain_hash(db)
        && let Some(best_chain_header) = load_header(db, &best_chain_hash)
    {
        best_chain_header.point()
    } else {
        Point::Origin
    };

    set_anchor_hash(db, &anchor_hash)?;
    set_best_chain_hash(db, &anchor_hash)?;
    set_block_valid(db, &anchor_hash, true)?;

    tracing::info!(prev_best_chain = %best_chain_point, new_best_chain = %anchor_point, "wound back best chain to revalidate");

    set_version(db, 3)?;
    Ok(())
}

// FIXME: this function and the following duplicate code in mod.rs The
// problem is that we need it to be polymorphic in the type of DB but
// we have 2 different incompatible types, DB and
// OptimisticTransactionDB.
fn load_header(db: &OptimisticTransactionDB, hash: &Hash<32>) -> Option<BlockHeader> {
    let prefix = [&HEADER_PREFIX[..], &hash[..]].concat();
    db.get_pinned(prefix).ok().and_then(|bytes| from_cbor(bytes?.as_ref()))
}

fn get_best_chain_hash(db: &OptimisticTransactionDB) -> Option<HeaderHash> {
    let bytes = db.get_pinned(BEST_CHAIN_PREFIX).ok().flatten()?;
    if bytes.len() == hash::size::HEADER { Some(Hash::from(bytes.as_ref())) } else { None }
}

fn get_anchor_hash(db: &OptimisticTransactionDB) -> Option<HeaderHash> {
    let bytes = db.get_pinned(ANCHOR_PREFIX).ok().flatten()?;
    if bytes.len() == hash::size::HEADER { Some(Hash::from(bytes.as_ref())) } else { None }
}

fn set_best_chain_hash(db: &OptimisticTransactionDB, hash: &HeaderHash) -> Result<(), StoreError> {
    db.put(BEST_CHAIN_PREFIX, hash.as_ref()).map_err(|e| StoreError::WriteError { error: e.to_string() })
}

fn set_anchor_hash(db: &OptimisticTransactionDB, hash: &HeaderHash) -> Result<(), StoreError> {
    db.put(ANCHOR_PREFIX, hash.as_ref()).map_err(|e| StoreError::WriteError { error: e.to_string() })
}

fn set_block_valid(db: &OptimisticTransactionDB, hash: &HeaderHash, valid: bool) -> Result<(), StoreError> {
    db.put([&HEADER_PREFIX[..], &hash[..], &[0]].concat(), [valid as u8])
        .map_err(|e| StoreError::WriteError { error: e.to_string() })
}

/// Check the version stored in the `db` matches `CHAIN_DB_VERSION`.
pub fn check_db_version(db: &OptimisticTransactionDB) -> Result<(), StoreError> {
    get_version(db).and_then(|stored| {
        if stored != CHAIN_DB_VERSION {
            Err(StoreError::IncompatibleChainStoreVersions { stored, current: CHAIN_DB_VERSION })
        } else {
            Ok(())
        }
    })
}

/// Retrieve the version of the Chain DB stored in the given `db`.
/// If no version is stored, returns 0.
pub fn get_version(db: &OptimisticTransactionDB) -> Result<u16, StoreError> {
    let raw_version = db.get(VERSION_KEY).map_err(|e| StoreError::OpenError { error: e.to_string() })?;

    match raw_version {
        None => Ok(0),
        Some(v) => match v.as_slice() {
            [v0, v1] => Ok(((*v0 as u16) << 8) | (*v1 as u16)),
            _ => Err(StoreError::OpenError { error: format!("Invalid __VERSION__ value length: {}", v.len()) }),
        },
    }
}

/// Set the version of the Chain DB stored in the given `db` to the
/// current `CHAIN_DB_VERSION`.
pub fn set_version(db: &OptimisticTransactionDB, version: u16) -> Result<(), StoreError> {
    let bytes = version.to_be_bytes();
    db.put(VERSION_KEY, bytes).map_err(|e| StoreError::WriteError { error: e.to_string() })
}
