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

use crate::rocksdb::RocksDbConfig;
use amaru_ouroboros_traits::StoreError;
use rocksdb::{OptimisticTransactionDB, Options};
use std::path::PathBuf;

pub(crate) const CONSENSUS_PREFIX_LEN: usize = 5;

/// Current version of chain DB format expected by this code, as a simple number.
/// Increment this number by 1 every time the "schema" is updated, eg. a new
/// type of keys is added, prefixes are changed, etc. then provide a migration
/// function from the previous version.
pub const CHAIN_DB_VERSION: u16 = 1;

pub(crate) const ANCHOR_PREFIX: [u8; CONSENSUS_PREFIX_LEN] = *b"ancho";
pub(crate) const BEST_CHAIN_PREFIX: [u8; CONSENSUS_PREFIX_LEN] = *b"best_";
pub(crate) const BLOCK_PREFIX: [u8; CONSENSUS_PREFIX_LEN] = *b"block";
pub(crate) const CHAIN_PREFIX: [u8; CONSENSUS_PREFIX_LEN] = *b"chain";
pub(crate) const CHILD_PREFIX: [u8; CONSENSUS_PREFIX_LEN] = *b"child";
pub(crate) const HEADER_PREFIX: [u8; CONSENSUS_PREFIX_LEN] = *b"heade";
pub(crate) const NONCES_PREFIX: [u8; CONSENSUS_PREFIX_LEN] = *b"nonce";

/// Open a Chain DB for reading and writing.
/// The DB _must_ exist otherwise the function will return an error.
pub fn open_db(config: &RocksDbConfig) -> Result<(PathBuf, OptimisticTransactionDB), StoreError> {
    let basedir = config.dir.clone();
    let mut opts: Options = config.into();
    opts.create_if_missing(false);
    let db = do_open_rocks_db(&basedir, opts)?;
    Ok((basedir, db))
}

/// Create or open a Chain DB for reading and writing.
/// The DB _may_ exist, if it does not it will be created.
pub fn open_or_create_db(
    config: &RocksDbConfig,
) -> Result<(PathBuf, OptimisticTransactionDB), StoreError> {
    let basedir = config.dir.clone();
    let mut opts: Options = config.into();
    opts.create_if_missing(true);
    let db = do_open_rocks_db(&basedir, opts)?;
    Ok((basedir, db))
}

fn do_open_rocks_db(
    basedir: &PathBuf,
    mut opts: Options,
) -> Result<OptimisticTransactionDB, StoreError> {
    opts.set_prefix_extractor(rocksdb::SliceTransform::create_fixed_prefix(
        CONSENSUS_PREFIX_LEN,
    ));
    let db = OptimisticTransactionDB::open(&opts, basedir).map_err(|e| StoreError::OpenError {
        error: e.to_string(),
    })?;
    Ok(db)
}
