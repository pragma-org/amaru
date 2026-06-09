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

use std::{fs, path::PathBuf};

use amaru_kernel::{HeaderHash, IsHeader as _};
use amaru_ouroboros_traits::{BaseReadChainStore, StoreError};
use rocksdb::{DB, Options, WriteBatch};

use crate::rocksdb::{
    RocksDbConfig,
    consensus::{
        DbOps, check_db_version, migrate_db, set_version,
        util::{BLOCK_PREFIX, CHAIN_DB_VERSION, CHILD_PREFIX, HEADER_PREFIX, open_db, open_or_create_db},
    },
};

pub struct RocksDBStore<T: DbOps = DB> {
    pub basedir: PathBuf,
    pub db: T,
}

impl RocksDBStore<DB> {
    /// Open an existing `RocksDBStore` with given configuration.
    ///
    /// This function will fail if:
    /// * the DB does not exist
    /// * the DB exists but with an incompatible version
    pub fn open(config: &RocksDbConfig) -> Result<Self, StoreError> {
        let (basedir, db) = open_db(config)?;
        let store = Self { db, basedir };
        check_db_version(&store)?;
        Ok(store)
    }

    /// Create a `RocksDBStore` with given configuration.
    /// If the database already exists, an error will be raised.
    /// To check the existence of the database we only check the directory pointed at
    /// contains at least one file.
    /// NOTE: There should be a better way to detect whether or not a directory contains
    /// a RocksDB database.
    pub fn create(config: RocksDbConfig) -> Result<Self, StoreError> {
        let basedir = config.dir.clone();
        let list = fs::read_dir(&basedir);
        if let Ok(entries) = list
            && entries.count() > 0
        {
            return Err(StoreError::OpenError {
                error: format!("Cannot create RocksDB at {}, directory is not empty", basedir.display()),
            });
        }

        let (_, db) = open_or_create_db(&config)?;
        let store = Self { db, basedir };
        set_version(&store, CHAIN_DB_VERSION)?;

        Ok(store)
    }

    /// Open or create a `RocksDBStore` with given configuration.
    ///
    /// This function is deemed "unsafe" because it automatically tries to migrate the
    /// DB it opens or creates which can potentially causes data corruption.
    pub fn open_and_migrate(config: &RocksDbConfig) -> Result<Self, StoreError> {
        let (basedir, db) = open_or_create_db(config)?;
        let store = Self { db, basedir };

        migrate_db(&store)?;

        Ok(store)
    }

    pub fn open_for_readonly(config: &RocksDbConfig) -> Result<Self, StoreError> {
        let basedir = config.dir.clone();
        let opts: Options = config.into();
        let db = DB::open_for_read_only(&opts, &basedir, false)
            .map_err(|e| StoreError::OpenError { error: e.to_string() })?;
        Ok(Self { db, basedir })
    }

    /// Runs the provided closure with a fresh `WriteBatch`, then commits it atomically.
    ///
    /// All puts/deletes accumulated in the batch are executed atomically (or not at all).
    /// If the closure short-circuits by returning `Err` then nothing is written.
    pub fn with_batch<F>(&self, f: F) -> Result<(), StoreError>
    where
        F: FnOnce(&mut WriteBatch) -> Result<(), StoreError>,
    {
        let mut batch = WriteBatch::default();
        f(&mut batch)?;
        self.db.write(batch).map_err(|e| StoreError::WriteError { error: e.to_string() })
    }

    pub fn remove_block_valid(&self, hash: &HeaderHash) -> Result<(), StoreError> {
        self.db
            .delete([&HEADER_PREFIX[..], &hash[..], &[0]].concat())
            .map_err(|e| StoreError::WriteError { error: e.to_string() })
    }

    pub fn remove_block(&self, hash: &HeaderHash) -> Result<(), StoreError> {
        self.with_batch(|batch| {
            batch.delete([&BLOCK_PREFIX[..], &hash[..]].concat());
            batch.delete([&HEADER_PREFIX[..], &hash[..], &[0]].concat());
            Ok(())
        })
    }

    pub fn remove_header(&self, hash: &HeaderHash) -> Result<(), StoreError> {
        let parent = self.load_header(hash).and_then(|h| h.parent());
        self.with_batch(|batch| {
            if let Some(parent) = parent {
                batch.delete([&CHILD_PREFIX[..], &parent[..], &hash[..]].concat());
            }
            batch.delete([&HEADER_PREFIX[..], &hash[..]].concat());
            batch.delete([&BLOCK_PREFIX[..], &hash[..]].concat());
            batch.delete([&HEADER_PREFIX[..], &hash[..], &[0]].concat());
            Ok(())
        })
    }
}
