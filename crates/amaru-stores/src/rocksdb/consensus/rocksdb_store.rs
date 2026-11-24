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
    fs,
    path::{Path, PathBuf},
};

use amaru_kernel::{HeaderHash, IsHeader as _, NetworkPoint, RawBlock, Slot, from_cbor, to_cbor};
use amaru_ouroboros_traits::{BaseReadChainStore, StoreError};
use rocksdb::{DB, Options, WriteBatch};

use crate::rocksdb::{
    RocksDbConfig,
    consensus::{
        DbOps,
        base_read_chain_store::opcert_key,
        check_db_version, migrate_db, set_version,
        util::{
            BLOCK_METADATA_PREFIX, BLOCK_PREFIX, CHAIN_DB_VERSION, CHILD_PREFIX, HEADER_PREFIX, NONCES_PREFIX,
            historical_block_key, open_db, open_or_create_db,
        },
    },
};

fn is_absent_or_empty_dir(path: &Path) -> bool {
    match fs::read_dir(path) {
        Err(_) => true,
        Ok(entries) => entries.count() == 0,
    }
}

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
    /// When the directory does not exist or is empty, creates a fresh database at the
    /// current `CHAIN_DB_VERSION` without replaying historical migrations.
    ///
    /// When the directory already contains a database, opens it and applies migrations
    /// up to the current version. This path is deemed "unsafe" because automatic
    /// migration can potentially cause data corruption.
    pub fn open_and_migrate(config: &RocksDbConfig) -> Result<Self, StoreError> {
        if is_absent_or_empty_dir(&config.dir) {
            return Self::create(config.clone());
        }

        let (basedir, db) = open_db(config)?;
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

    /// Store a bounded batch from a verified archive for serving to peers, including range links,
    /// without changing headers or chain selection. Blocks remain opaque in storage;
    /// ledger callers must check their integrity before processing them.
    /// Blocks must be in archive order; the last block at each slot is indexed.
    pub fn store_historical_blocks(
        &self,
        blocks: &[(NetworkPoint, Option<HeaderHash>, RawBlock)],
    ) -> Result<(), StoreError> {
        self.with_batch(|batch| {
            for (point, parent, block) in blocks {
                let hash = point.hash();
                batch.put([&BLOCK_PREFIX[..], &hash[..]].concat(), block.as_ref());
                batch
                    .put([&BLOCK_METADATA_PREFIX[..], &hash[..]].concat(), to_cbor(&(point.slot_or_default(), parent)));
                let slot = point.slot_or_default();
                if let Some(current) = self.read_historical_block(slot)?
                    && self.read_block_metadata(&current.hash())?.is_some_and(|(_, parent)| parent == Some(hash))
                {
                    continue;
                }
                batch.put(historical_block_key(slot), to_cbor(point));
            }
            Ok(())
        })
    }

    pub fn remove_block(&self, hash: &HeaderHash) -> Result<(), StoreError> {
        self.with_batch(|batch| {
            self.remove_historical_block_index(batch, hash)?;
            batch.delete([&BLOCK_PREFIX[..], &hash[..]].concat());
            batch.delete([&BLOCK_METADATA_PREFIX[..], &hash[..]].concat());
            batch.delete([&HEADER_PREFIX[..], &hash[..], &[0]].concat());
            Ok(())
        })
    }

    pub fn remove_header(&self, hash: &HeaderHash) -> Result<(), StoreError> {
        let parent = self.load_header(hash).and_then(|h| h.parent());
        let header = self.load_header(hash);
        self.with_batch(|batch| {
            self.remove_historical_block_index(batch, hash)?;
            if let Some(parent) = parent {
                batch.delete([&CHILD_PREFIX[..], &parent[..], &hash[..]].concat());
            }
            if let Some(header) = &header {
                if let Some(parent) = header.parent() {
                    batch.delete([&CHILD_PREFIX[..], &parent[..], &hash[..]].concat());
                }
                batch.delete(opcert_key(header));
            }
            batch.delete([&HEADER_PREFIX[..], &hash[..]].concat());
            batch.delete([&BLOCK_PREFIX[..], &hash[..]].concat());
            batch.delete([&BLOCK_METADATA_PREFIX[..], &hash[..]].concat());
            batch.delete([&HEADER_PREFIX[..], &hash[..], &[0]].concat());
            batch.delete([&NONCES_PREFIX[..], &hash[..]].concat());
            Ok(())
        })
    }

    fn read_historical_block(&self, slot: Slot) -> Result<Option<NetworkPoint>, StoreError> {
        self.db
            .get_pinned(historical_block_key(slot))
            .map_err(|e| StoreError::ReadError { error: e.to_string() })?
            .map(|bytes| {
                from_cbor(bytes.as_ref())
                    .ok_or_else(|| StoreError::ReadError { error: "invalid historical block point".into() })
            })
            .transpose()
    }

    fn remove_historical_block_index(&self, batch: &mut WriteBatch, hash: &HeaderHash) -> Result<(), StoreError> {
        if let Some((slot, parent)) = self.read_block_metadata(hash)?
            && self.read_historical_block(slot)? == Some(NetworkPoint::Specific(slot, *hash))
        {
            let key = historical_block_key(slot);
            batch.delete(&key);
            if let Some(parent) = parent
                && self.read_block_metadata(&parent)?.is_some_and(|(parent_slot, _)| parent_slot == slot)
            {
                batch.put(key, to_cbor(&NetworkPoint::Specific(slot, parent)));
            }
        }
        Ok(())
    }
}
