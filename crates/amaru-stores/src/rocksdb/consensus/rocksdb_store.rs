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
use rocksdb::{DB, OptimisticTransactionDB, Options};

use crate::rocksdb::{
    RocksDbConfig,
    consensus::{
        DbOps, check_db_version, migrate_db, set_version,
        util::{BLOCK_PREFIX, CHAIN_DB_VERSION, CHILD_PREFIX, HEADER_PREFIX, open_db, open_or_create_db},
    },
};

pub struct RocksDBStore<T: DbOps = OptimisticTransactionDB> {
    pub basedir: PathBuf,
    pub db: T,
}

impl RocksDBStore<OptimisticTransactionDB> {
    /// Open an existing `RocksDBStore` with given configuration.
    ///
    /// This function will fail if:
    /// * the DB does not exist
    /// * the DB exists but with an incompatible version
    pub fn open(config: &RocksDbConfig) -> Result<Self, StoreError> {
        let (basedir, db) = open_db(config)?;
        check_db_version(&db)?;
        Ok(Self { db, basedir })
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
        set_version(&db, CHAIN_DB_VERSION)?;

        Ok(Self { db, basedir })
    }

    /// Open or create a `RocksDBStore` with given configuration.
    ///
    /// This function is deemed "unsafe" because it automatically tries to migrate the
    /// DB it opens or creates which can potentially causes data corruption.
    pub fn open_and_migrate(config: &RocksDbConfig) -> Result<Self, StoreError> {
        let (basedir, db) = open_or_create_db(config)?;

        migrate_db(&db)?;

        Ok(Self { db, basedir })
    }

    pub fn create_transaction(&self) -> rocksdb::Transaction<'_, OptimisticTransactionDB> {
        self.db.transaction()
    }

    /// Runs the provided closure within a transaction.
    ///
    /// The transaction is committed if the closure returns `Ok`, otherwise it is rolled back.
    /// Note the `commit` itself can also fail which is reported as a `StoreError::WriteError`.
    pub fn with_transaction<R, F>(&self, f: F) -> Result<R, StoreError>
    where
        F: FnOnce(&rocksdb::Transaction<'_, OptimisticTransactionDB>) -> Result<R, StoreError>,
    {
        let tx = self.db.transaction();
        match f(&tx) {
            Ok(result) => {
                tx.commit().map_err(|e| StoreError::WriteError { error: e.to_string() })?;
                Ok(result)
            }
            Err(err) => Err(err),
        }
    }

    pub fn remove_block_valid(&self, hash: &HeaderHash) -> Result<(), StoreError> {
        self.db
            .delete([&HEADER_PREFIX[..], &hash[..], &[0]].concat())
            .map_err(|e| StoreError::WriteError { error: e.to_string() })
    }

    pub fn remove_block(&self, hash: &HeaderHash) -> Result<(), StoreError> {
        self.db
            .delete([&BLOCK_PREFIX[..], &hash[..]].concat())
            .map_err(|e| StoreError::WriteError { error: e.to_string() })?;
        self.remove_block_valid(hash)?;
        Ok(())
    }

    pub fn remove_header(&self, hash: &HeaderHash) -> Result<(), StoreError> {
        let parent = self.load_header(hash).and_then(|h| h.parent());
        if let Some(parent) = parent {
            self.db
                .delete([&CHILD_PREFIX[..], &parent[..], &hash[..]].concat())
                .map_err(|e| StoreError::WriteError { error: e.to_string() })?;
        }
        self.db
            .delete([&HEADER_PREFIX[..], &hash[..]].concat())
            .map_err(|e| StoreError::WriteError { error: e.to_string() })?;
        self.remove_block(hash)?;
        Ok(())
    }
}

impl RocksDBStore<DB> {
    pub fn open_for_readonly(config: &RocksDbConfig) -> Result<Self, StoreError> {
        let basedir = config.dir.clone();
        let opts: Options = config.into();
        let db = DB::open_for_read_only(&opts, &basedir, false)
            .map_err(|e| StoreError::OpenError { error: e.to_string() })?;
        Ok(Self { db, basedir })
    }
}
