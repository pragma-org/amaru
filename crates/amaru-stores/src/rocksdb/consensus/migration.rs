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
    consensus::util::{CHAIN_DB_VERSION, open_db},
};
use amaru_ouroboros_traits::StoreError;
use rocksdb::OptimisticTransactionDB;
use std::path::PathBuf;
use tracing::info;

/// The version key: __VERSION__
pub const VERSION_KEY: [u8; 11] = [
    0x5f, 0x5f, 0x56, 0x45, 0x52, 0x53, 0x49, 0x4f, 0x4e, 0x5f, 0x5f,
];

/// Migrate the Chain Database at the given `path` to the current `CHAIN_DB_VERSION`.
/// Returns the pair of numbers consisting in the initial version of the database and
/// the current version if migration succeeds, otherwise returns a `StoreError`.
pub fn migrate_db(path: &PathBuf) -> Result<(u16, u16), StoreError> {
    let config = RocksDbConfig::new(path.to_path_buf());

    let (_, db) = open_db(&config)?;

    let version = get_version(&db)?;

    for n in version..CHAIN_DB_VERSION {
        info!("Migrating Chain database to version {}", n + 1);
        MIGRATIONS[n as usize](&db)?
    }
    Ok((version, CHAIN_DB_VERSION))
}

/// "Migrate" DB to version 1
/// This simply records the VERSIION_KEY into the db.
fn migrate_to_v1(db: &OptimisticTransactionDB) -> Result<(), StoreError> {
    db.put(VERSION_KEY, [0x0, 0x1])
        .map_err(|e| StoreError::WriteError {
            error: e.to_string(),
        })
}

/// List of migrations to apply, in order.
/// Each function at index `i` in this array corresponds to a migration from version `i` to version `i + 1`.
/// When modifying the DB schema, create migration function and add it to this array bumping its length.
static MIGRATIONS: [fn(&OptimisticTransactionDB) -> Result<(), StoreError>; 1] = [migrate_to_v1];

fn get_version(db: &OptimisticTransactionDB) -> Result<u16, StoreError> {
    let raw_version = db.get(VERSION_KEY).map_err(|e| StoreError::OpenError {
        error: e.to_string(),
    })?;

    Ok(raw_version
        .map(|v| match v.as_slice() {
            [v0, v1] => ((*v0 as u16) << 8) | *v1 as u16,
            _ => 0,
        })
        .unwrap_or(0))
}
