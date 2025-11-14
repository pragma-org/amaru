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

use crate::rocksdb::{
    RocksDbConfig,
    consensus::{
        store_chain_point,
        util::{BEST_CHAIN_PREFIX, CHAIN_DB_VERSION, HEADER_PREFIX, open_db},
    },
};
use amaru_kernel::{BlockHeader, HEADER_HASH_SIZE, Hash, HeaderHash, IsHeader, from_cbor};
use amaru_ouroboros_traits::StoreError;
use rocksdb::OptimisticTransactionDB;
use std::path::Path;
use tracing::info;

/// The version key: __VERSION__
pub const VERSION_KEY: [u8; 11] = [
    0x5f, 0x5f, 0x56, 0x45, 0x52, 0x53, 0x49, 0x4f, 0x4e, 0x5f, 0x5f,
];

/// Migrate the Chain Database at the given `path` to the current `CHAIN_DB_VERSION`.
/// Returns the pair of numbers consisting in the initial version of the database and
/// the current version if migration succeeds, otherwise returns a `StoreError`.
pub fn migrate_db_path(path: &Path) -> Result<(u16, u16), StoreError> {
    let config = RocksDbConfig::new(path.to_path_buf());

    let (_, db) = open_db(&config)?;

    migrate_db(&db)
}

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
    db.put(VERSION_KEY, [0x0, 0x1])
        .map_err(|e| StoreError::WriteError {
            error: e.to_string(),
        })
}

/// "Migrate" DB to version 2
/// Walks the best chain backwards and re-inserts all points.
fn migrate_to_v2(db: &OptimisticTransactionDB) -> Result<(), StoreError> {
    db.put(VERSION_KEY, [0x0, 0x2])
        .map_err(|e| StoreError::WriteError {
            error: e.to_string(),
        })?;

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

    Ok(())
}

// FIXME: this function and the following duplicate code in mod.rs The
// problem is that we need it to be polymorphic in the type of DB but
// we have 2 different incompatible types, DB and
// OptimisticTransactionDB.
fn load_header(db: &OptimisticTransactionDB, hash: &Hash<32>) -> Option<BlockHeader> {
    let prefix = [&HEADER_PREFIX[..], &hash[..]].concat();
    db.get_pinned(prefix)
        .ok()
        .and_then(|bytes| from_cbor(bytes?.as_ref()))
}

fn get_best_chain_hash(db: &OptimisticTransactionDB) -> Option<HeaderHash> {
    db.get_pinned(BEST_CHAIN_PREFIX)
        .ok()
        .flatten()
        .and_then(|bytes| {
            if bytes.len() == HEADER_HASH_SIZE {
                Some(Hash::from(bytes.as_ref()))
            } else {
                None
            }
        })
}

/// List of migrations to apply, in order.
///
/// Each function at index `i` in this array corresponds to a
/// migration from version `i` to version `i + 1`.  When modifying the
/// DB schema, create migration function and add it to this array
/// bumping its length.
static MIGRATIONS: [fn(&OptimisticTransactionDB) -> Result<(), StoreError>; 2] =
    [migrate_to_v1, migrate_to_v2];

/// Check the version stored in the `db` matches `CHAIN_DB_VERSION`.
pub fn check_db_version(db: &OptimisticTransactionDB) -> Result<(), StoreError> {
    get_version(db).and_then(|stored| {
        if stored != CHAIN_DB_VERSION {
            Err(StoreError::IncompatibleChainStoreVersions {
                stored,
                current: CHAIN_DB_VERSION,
            })
        } else {
            Ok(())
        }
    })
}

/// Retrieve the version of the Chain DB stored in the given `db`.
/// If no version is stored, returns 0.
pub fn get_version(db: &OptimisticTransactionDB) -> Result<u16, StoreError> {
    let raw_version = db.get(VERSION_KEY).map_err(|e| StoreError::OpenError {
        error: e.to_string(),
    })?;

    match raw_version {
        None => Ok(0),
        Some(v) => match v.as_slice() {
            [v0, v1] => Ok(((*v0 as u16) << 8) | (*v1 as u16)),
            _ => Err(StoreError::OpenError {
                error: format!("Invalid __VERSION__ value length: {}", v.len()),
            }),
        },
    }
}

/// Set the version of the Chain DB stored in the given `db` to the
/// current `CHAIN_DB_VERSION`.
pub fn set_version(db: &OptimisticTransactionDB) -> Result<(), StoreError> {
    let bytes: Vec<u8> = vec![
        (CHAIN_DB_VERSION >> 8) as u8,
        (CHAIN_DB_VERSION & 0xff) as u8,
    ];
    db.put(VERSION_KEY, &bytes)
        .map_err(|e| StoreError::WriteError {
            error: e.to_string(),
        })
}
