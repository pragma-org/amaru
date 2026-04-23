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

use std::{fs, path::PathBuf};

use amaru_kernel::{
    BlockHeader, Hash, HeaderHash, IsHeader, NonEmptyVec, ORIGIN_HASH, Point, RawBlock, cbor, from_cbor, size::HEADER,
    to_cbor,
};
use amaru_observability::trace_span;
use amaru_ouroboros_traits::{ChainStore, DiagnosticChainStore, Nonces, ReadOnlyChainStore, StoreError};
use rocksdb::{
    DB, DBCommon, DBIteratorWithThreadMode, DBPinnableSlice, IteratorMode, OptimisticTransactionDB, Options,
    PrefixRange, ReadOptions, SnapshotWithThreadMode,
};

use crate::rocksdb::{
    RocksDbConfig,
    consensus::util::{
        ANCHOR_PREFIX, BEST_CHAIN_PREFIX, BLOCK_PREFIX, CHAIN_DB_VERSION, CHAIN_PREFIX, CHILD_PREFIX,
        CONSENSUS_PREFIX_LEN, HEADER_PREFIX, NONCES_PREFIX, open_db, open_or_create_db,
    },
};

pub mod migration;
pub mod util;

pub use migration::*;

pub trait DbOps: rocksdb::DBAccess + Sized {
    fn get_pinned(&self, key: &[u8], opts: &ReadOptions) -> Result<Option<DBPinnableSlice<'_>>, StoreError>;
    fn multi_get(&self, keys: &[&[u8]], opts: &ReadOptions) -> Vec<Result<Option<Vec<u8>>, StoreError>>;
    fn prefix_iterator(&self, prefix: &[u8]) -> DBIteratorWithThreadMode<'_, Self>;
    fn iterator_opt(&self, mode: IteratorMode<'_>, opts: ReadOptions) -> DBIteratorWithThreadMode<'_, Self>;
}
impl DbOps for OptimisticTransactionDB {
    fn get_pinned(&self, key: &[u8], opts: &ReadOptions) -> Result<Option<DBPinnableSlice<'_>>, StoreError> {
        DBCommon::get_pinned_opt(self, key, opts).map_err(|e| StoreError::ReadError { error: e.to_string() })
    }

    fn multi_get(&self, keys: &[&[u8]], opts: &ReadOptions) -> Vec<Result<Option<Vec<u8>>, StoreError>> {
        DBCommon::multi_get_opt(self, keys, opts)
            .into_iter()
            .map(|result| result.map_err(|e| StoreError::ReadError { error: e.to_string() }))
            .collect()
    }

    fn prefix_iterator(&self, prefix: &[u8]) -> DBIteratorWithThreadMode<'_, Self> {
        DBCommon::prefix_iterator(self, prefix)
    }

    fn iterator_opt(&self, mode: IteratorMode<'_>, opts: ReadOptions) -> DBIteratorWithThreadMode<'_, Self> {
        DBCommon::iterator_opt(self, mode, opts)
    }
}
impl DbOps for DB {
    fn get_pinned(&self, key: &[u8], opts: &ReadOptions) -> Result<Option<DBPinnableSlice<'_>>, StoreError> {
        DBCommon::get_pinned_opt(self, key, opts).map_err(|e| StoreError::ReadError { error: e.to_string() })
    }

    fn multi_get(&self, keys: &[&[u8]], opts: &ReadOptions) -> Vec<Result<Option<Vec<u8>>, StoreError>> {
        DBCommon::multi_get_opt(self, keys, opts)
            .into_iter()
            .map(|result| result.map_err(|e| StoreError::ReadError { error: e.to_string() }))
            .collect()
    }

    fn prefix_iterator(&self, prefix: &[u8]) -> DBIteratorWithThreadMode<'_, Self> {
        DBCommon::prefix_iterator(self, prefix)
    }

    fn iterator_opt(&self, mode: IteratorMode<'_>, opts: ReadOptions) -> DBIteratorWithThreadMode<'_, Self> {
        DBCommon::iterator_opt(self, mode, opts)
    }
}

pub struct RocksDBStore<T: DbOps = OptimisticTransactionDB> {
    pub basedir: PathBuf,
    pub db: T,
}

/// Read-only view of the chain store backed by a RocksDB snapshot so related reads stay consistent.
struct RocksDBSnapshot<'a> {
    db: &'a OptimisticTransactionDB,
    snapshot: SnapshotWithThreadMode<'a, OptimisticTransactionDB>,
}

impl RocksDBSnapshot<'_> {
    fn read_options(&self) -> ReadOptions {
        let mut opts = ReadOptions::default();
        opts.set_snapshot(&self.snapshot);
        opts
    }
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

pub(crate) fn store_chain_point(db: &OptimisticTransactionDB, point: &Point) -> Result<(), StoreError> {
    let slot = u64::from(point.slot_or_default()).to_be_bytes();
    db.put([&CHAIN_PREFIX[..], &slot[..]].concat(), point.hash().as_ref())
        .map_err(|e| StoreError::WriteError { error: e.to_string() })
}

impl<H> ReadOnlyChainStore<H> for RocksDBSnapshot<'_>
where
    H: IsHeader + Clone + for<'d> cbor::Decode<'d, ()>,
{
    fn load_header(&self, hash: &HeaderHash) -> Option<H> {
        let prefix = [&HEADER_PREFIX[..], &hash[..]].concat();
        let opts = self.read_options();
        DbOps::get_pinned(self.db, &prefix, &opts).ok().flatten().and_then(|bytes| from_cbor(bytes.as_ref()))
    }

    fn load_header_with_validity(&self, hash: &HeaderHash) -> Option<(H, Option<bool>)> {
        let prefix = [&HEADER_PREFIX[..], &hash[..], &[0]].concat();
        let head_len = prefix.len() - 1;
        let opts = self.read_options();
        let mut results = DbOps::multi_get(self.db, &[&prefix[..head_len], &prefix], &opts).into_iter();
        let header = results.next().and_then(|bytes| from_cbor(bytes.ok()??.as_ref()));
        let validity = results.next().and_then(|bytes| {
            let bytes = bytes.ok()??;
            if bytes.len() == 1 { Some(bytes[0] == 1) } else { None }
        });
        header.map(|h| (h, validity))
    }

    fn get_children(&self, hash: &HeaderHash) -> Vec<HeaderHash> {
        let mut result = Vec::new();
        let mut opts = self.read_options();
        opts.set_iterate_range(PrefixRange([&CHILD_PREFIX[..], &hash[..]].concat()));

        for res in self.db.iterator_opt(IteratorMode::Start, opts) {
            // FIXME: RocksDB iterator errors (transient I/O, corruption) panic the node here.
            // Propagating as StoreError requires changing the `get_children` trait signature
            // across ReadOnlyChainStore and all impls/callers. Tracked as follow-up.
            #[expect(clippy::expect_used)]
            let (key, _value) = res.expect("error iterating over children");
            let mut arr = [0u8; HEADER];
            arr.copy_from_slice(&key[(CONSENSUS_PREFIX_LEN + HEADER)..]);
            result.push(Hash::from(arr));
        }
        result
    }

    fn get_anchor_hash(&self) -> HeaderHash {
        let opts = self.read_options();
        self.db
            .get_pinned_opt(ANCHOR_PREFIX, &opts)
            .ok()
            .flatten()
            .and_then(|bytes| if bytes.len() == HEADER { Some(Hash::from(bytes.as_ref())) } else { None })
            .unwrap_or(ORIGIN_HASH)
    }

    fn get_best_chain_hash(&self) -> HeaderHash {
        let opts = self.read_options();
        self.db
            .get_pinned_opt(BEST_CHAIN_PREFIX, &opts)
            .ok()
            .flatten()
            .and_then(|bytes| if bytes.len() == HEADER { Some(Hash::from(bytes.as_ref())) } else { None })
            .unwrap_or(ORIGIN_HASH)
    }

    fn has_header(&self, hash: &HeaderHash) -> bool {
        let prefix = [&HEADER_PREFIX[..], &hash[..]].concat();
        let opts = self.read_options();
        DbOps::get_pinned(self.db, &prefix, &opts).map(|opt| opt.is_some()).unwrap_or(false)
    }

    fn get_nonces(&self, header: &HeaderHash) -> Option<Nonces> {
        let opts = self.read_options();
        self.db
            .get_pinned_opt([&NONCES_PREFIX[..], &header[..]].concat(), &opts)
            .ok()
            .flatten()
            .as_deref()
            .and_then(from_cbor)
    }

    fn load_block(&self, hash: &HeaderHash) -> Result<Option<RawBlock>, StoreError> {
        let opts = self.read_options();
        Ok(DbOps::get_pinned(self.db, &[&BLOCK_PREFIX[..], &hash[..]].concat(), &opts)?
            .map(|bytes| bytes.as_ref().into()))
    }

    fn load_from_best_chain(&self, point: &Point) -> Option<HeaderHash> {
        let slot = u64::from(point.slot_or_default()).to_be_bytes();
        let opts = self.read_options();
        DbOps::get_pinned(self.db, &[&CHAIN_PREFIX[..], &slot[..]].concat(), &opts).ok().flatten().and_then(|bytes| {
            if bytes.len() == HEADER {
                let hash = Hash::from(bytes.as_ref());
                if *hash == *point.hash() { Some(hash) } else { None }
            } else {
                None
            }
        })
    }

    fn next_best_chain(&self, point: &Point) -> Option<Point> {
        let mut readopts = self.read_options();
        readopts.set_iterate_range(PrefixRange(CHAIN_PREFIX));
        let prefix = [&CHAIN_PREFIX[..], &(u64::from(point.slot_or_default()) + 1).to_be_bytes()].concat();
        let mut iter = self.db.iterator_opt(IteratorMode::From(&prefix, rocksdb::Direction::Forward), readopts);

        if let Some(Ok((k, v))) = iter.next() {
            #[expect(clippy::unwrap_used)]
            let slot_bytes: [u8; 8] = k[CHAIN_PREFIX.len()..CHAIN_PREFIX.len() + 8].try_into().unwrap();
            let slot = u64::from_be_bytes(slot_bytes);
            if v.len() == HEADER {
                let hash = <HeaderHash>::from(v.as_ref());
                Some(Point::Specific(slot.into(), hash))
            } else {
                None
            }
        } else {
            None
        }
    }
}

impl<H, T: DbOps> ReadOnlyChainStore<H> for RocksDBStore<T>
where
    H: IsHeader + Clone + for<'d> cbor::Decode<'d, ()>,
{
    fn load_header(&self, hash: &HeaderHash) -> Option<H> {
        let prefix = [&HEADER_PREFIX[..], &hash[..]].concat();
        self.db.get_pinned_opt(prefix, &ReadOptions::default()).ok().and_then(|bytes| from_cbor(bytes?.as_ref()))
    }

    fn load_header_with_validity(&self, hash: &HeaderHash) -> Option<(H, Option<bool>)> {
        let prefix = [&HEADER_PREFIX[..], &hash[..], &[0]].concat();
        let head_len = prefix.len() - 1;
        let mut results = self.db.multi_get_opt([&prefix[..head_len], &prefix], &ReadOptions::default()).into_iter();
        let header = results.next().and_then(|bytes| from_cbor(bytes.ok()??.as_ref()));
        let validity = results.next().and_then(|bytes| {
            let bytes = bytes.ok()??;
            if bytes.len() == 1 { Some(bytes[0] == 1) } else { None }
        });
        header.map(|h| (h, validity))
    }

    fn get_children(&self, hash: &HeaderHash) -> Vec<HeaderHash> {
        let mut result = Vec::new();
        let mut opts = ReadOptions::default();
        opts.set_iterate_range(PrefixRange([&CHILD_PREFIX[..], &hash[..]].concat()));

        for res in self.db.iterator_opt(IteratorMode::Start, opts) {
            // FIXME: RocksDB iterator errors (transient I/O, corruption) panic the node here.
            // Propagating as StoreError requires changing the `get_children` trait signature
            // across ReadOnlyChainStore and all impls/callers. Tracked as follow-up.
            #[expect(clippy::expect_used)]
            let (key, _value) = res.expect("error iterating over children");
            let mut arr = [0u8; HEADER];
            arr.copy_from_slice(&key[(CONSENSUS_PREFIX_LEN + HEADER)..]);
            result.push(Hash::from(arr));
        }
        result
    }

    fn get_anchor_hash(&self) -> HeaderHash {
        self.db
            .get_pinned(&ANCHOR_PREFIX, &ReadOptions::default())
            .ok()
            .flatten()
            .and_then(|bytes| if bytes.len() == HEADER { Some(Hash::from(bytes.as_ref())) } else { None })
            .unwrap_or(ORIGIN_HASH)
    }

    fn get_best_chain_hash(&self) -> HeaderHash {
        self.db
            .get_pinned(&BEST_CHAIN_PREFIX, &ReadOptions::default())
            .ok()
            .flatten()
            .and_then(|bytes| if bytes.len() == HEADER { Some(Hash::from(bytes.as_ref())) } else { None })
            .unwrap_or(ORIGIN_HASH)
    }

    fn has_header(&self, hash: &HeaderHash) -> bool {
        let prefix = [&HEADER_PREFIX[..], &hash[..]].concat();
        self.db.get_pinned(&prefix, &ReadOptions::default()).map(|opt| opt.is_some()).unwrap_or(false)
    }

    fn get_nonces(&self, header: &HeaderHash) -> Option<Nonces> {
        self.db
            .get_pinned(&[&NONCES_PREFIX[..], &header[..]].concat(), &ReadOptions::default())
            .ok()
            .flatten()
            .as_deref()
            .and_then(from_cbor)
    }

    fn load_block(&self, hash: &HeaderHash) -> Result<Option<RawBlock>, StoreError> {
        Ok(self
            .db
            .get_pinned(&[&BLOCK_PREFIX[..], &hash[..]].concat(), &ReadOptions::default())?
            .map(|bytes| bytes.as_ref().into()))
    }

    fn load_from_best_chain(&self, point: &Point) -> Option<HeaderHash> {
        let slot = u64::from(point.slot_or_default()).to_be_bytes();
        self.db.get_pinned(&[&CHAIN_PREFIX[..], &slot[..]].concat(), &ReadOptions::default()).ok().flatten().and_then(
            |bytes| {
                if bytes.len() == HEADER {
                    let hash = Hash::from(bytes.as_ref());
                    if *hash == *point.hash() { Some(hash) } else { None }
                } else {
                    None
                }
            },
        )
    }

    fn next_best_chain(&self, point: &Point) -> Option<Point> {
        let mut readopts = ReadOptions::default();
        readopts.set_iterate_range(PrefixRange(CHAIN_PREFIX));
        let prefix = [&CHAIN_PREFIX[..], &(u64::from(point.slot_or_default()) + 1).to_be_bytes()].concat();
        let mut iter = self.db.iterator_opt(IteratorMode::From(&prefix, rocksdb::Direction::Forward), readopts);

        if let Some(Ok((k, v))) = iter.next() {
            #[expect(clippy::unwrap_used)]
            let slot_bytes: [u8; 8] = k[CHAIN_PREFIX.len()..CHAIN_PREFIX.len() + 8].try_into().unwrap();
            let slot = u64::from_be_bytes(slot_bytes);
            if v.len() == HEADER {
                let hash = <HeaderHash>::from(v.as_ref());
                Some(Point::Specific(slot.into(), hash))
            } else {
                None
            }
        } else {
            None
        }
    }
}

impl DiagnosticChainStore for RocksDBStore<DB> {
    #[allow(clippy::panic)]
    fn load_headers(&self) -> Box<dyn Iterator<Item = BlockHeader> + '_> {
        Box::new(self.db.prefix_iterator(HEADER_PREFIX).filter_map(|item| match item {
            Ok((_k, v)) => from_cbor(v.as_ref()),
            Err(err) => panic!("error iterating over headers: {}", err),
        }))
    }

    #[allow(clippy::panic)]
    fn load_nonces(&self) -> Box<dyn Iterator<Item = (HeaderHash, Nonces)> + '_> {
        Box::new(self.db.prefix_iterator(NONCES_PREFIX).filter_map(|item| match item {
            Ok((k, v)) => {
                let hash = Hash::from(&k[CONSENSUS_PREFIX_LEN..]);
                from_cbor(&v).map(|nonces| (hash, nonces))
            }
            Err(err) => panic!("error iterating over nonces: {}", err),
        }))
    }

    #[allow(clippy::panic)]
    fn load_blocks(&self) -> Box<dyn Iterator<Item = (HeaderHash, RawBlock)> + '_> {
        let mut opts = ReadOptions::default();
        opts.set_iterate_range(PrefixRange(&BLOCK_PREFIX[..]));
        Box::new(self.db.iterator_opt(IteratorMode::Start, opts).map(|item| match item {
            Ok((k, v)) => {
                let hash = Hash::from(&k[CONSENSUS_PREFIX_LEN..]);
                (hash, RawBlock::from(v))
            }
            Err(err) => panic!("error iterating over blocks: {}", err),
        }))
    }

    #[allow(clippy::expect_used)]
    fn load_parents_children(&self) -> Box<dyn Iterator<Item = (HeaderHash, Vec<HeaderHash>)> + '_> {
        let mut groups: Vec<(HeaderHash, Vec<HeaderHash>)> = Vec::new();
        let mut current_parent: Option<HeaderHash> = None;
        let mut current_children: Vec<HeaderHash> = Vec::new();
        let mut opts = ReadOptions::default();
        opts.set_iterate_range(PrefixRange(&CHILD_PREFIX[..]));

        for kv in self.db.iterator_opt(IteratorMode::Start, opts) {
            // FIXME: same as `get_children`; iterator errors panic the node. Diagnostic-only
            // path today, but worth unifying when the trait-level fix lands.
            let (k, _v) = kv.expect("error iterating over children keys");

            //Key layout: [CHILD_PREFIX][parent][child]
            let parent_start = CONSENSUS_PREFIX_LEN;
            let parent_end = parent_start + HEADER;
            let child_start = parent_end;
            let child_end = child_start + HEADER;

            let mut parent_arr = [0u8; HEADER];
            parent_arr.copy_from_slice(&k[parent_start..parent_end]);
            let parent_hash = Hash::from(parent_arr);

            let mut child_arr = [0u8; HEADER];

            child_arr.copy_from_slice(&k[child_start..child_end]);
            let child_hash = Hash::from(child_arr);

            match &current_parent {
                Some(p) if p == &parent_hash => {
                    current_children.push(child_hash);
                }
                Some(prev_parent) => {
                    groups.push((*prev_parent, std::mem::take(&mut current_children)));
                    current_parent = Some(parent_hash);
                    current_children.push(child_hash);
                }
                None => {
                    current_parent = Some(parent_hash);
                    current_children.push(child_hash);
                }
            }
        }

        if let Some(p) = current_parent {
            groups.push((p, current_children));
        }

        Box::new(groups.into_iter())
    }
}

use std::fmt::Debug;
impl<H: IsHeader + Clone + Debug + for<'d> cbor::Decode<'d, ()>> ChainStore<H> for RocksDBStore {
    fn snapshot(&self) -> Box<dyn ReadOnlyChainStore<H> + '_> {
        Box::new(RocksDBSnapshot { db: &self.db, snapshot: self.db.snapshot() })
    }

    fn store_header(&self, header: &H) -> Result<(), StoreError> {
        let _span = trace_span!(
            amaru_observability::amaru::stores::consensus::STORE_HEADER,
            hash = header.hash(),
            db_system_name = "rocksdb".to_string(),
            db_operation_name = "put".to_string(),
            db_collection_name = "header".to_string()
        );
        let _guard = _span.enter();

        let hash = header.hash();
        let tx = self.db.transaction();
        if let Some(parent) = header.parent() {
            tx.put([&CHILD_PREFIX[..], &parent[..], &hash[..]].concat(), [])
                .map_err(|e| StoreError::WriteError { error: e.to_string() })?;
        };
        tx.put([&HEADER_PREFIX[..], &hash[..]].concat(), to_cbor(header))
            .map_err(|e| StoreError::WriteError { error: e.to_string() })?;
        tx.commit().map_err(|e| StoreError::WriteError { error: e.to_string() })
    }

    fn set_block_valid(&self, hash: &HeaderHash, valid: bool) -> Result<(), StoreError> {
        self.db
            .put([&HEADER_PREFIX[..], &hash[..], &[0]].concat(), [valid as u8])
            .map_err(|e| StoreError::WriteError { error: e.to_string() })
    }

    fn put_nonces(&self, header: &HeaderHash, nonces: &Nonces) -> Result<(), StoreError> {
        self.db
            .put([&NONCES_PREFIX[..], &header[..]].concat(), to_cbor(nonces))
            .map_err(|e| StoreError::WriteError { error: e.to_string() })
    }

    fn store_block(&self, hash: &HeaderHash, block: &RawBlock) -> Result<(), StoreError> {
        let _span = trace_span!(
            amaru_observability::amaru::stores::consensus::STORE_BLOCK,
            hash = *hash,
            db_system_name = "rocksdb".to_string(),
            db_operation_name = "put".to_string(),
            db_collection_name = "block".to_string()
        );
        let _guard = _span.enter();

        self.db
            .put([&BLOCK_PREFIX[..], &hash[..]].concat(), block.as_ref())
            .map_err(|e| StoreError::WriteError { error: e.to_string() })
    }

    fn set_anchor_hash(&self, hash: &HeaderHash) -> Result<(), StoreError> {
        self.db.put(ANCHOR_PREFIX, hash.as_ref()).map_err(|e| StoreError::WriteError { error: e.to_string() })
    }

    fn set_best_chain_hash(&self, hash: &HeaderHash) -> Result<(), StoreError> {
        self.db.put(BEST_CHAIN_PREFIX, hash.as_ref()).map_err(|e| StoreError::WriteError { error: e.to_string() })
    }

    fn switch_to_fork(&self, fork_point: &Point, forward_points: &NonEmptyVec<Point>) -> Result<(), StoreError> {
        let last = forward_points.last();
        let _span = trace_span!(
            amaru_observability::amaru::stores::consensus::SWITCH_TO_FORK,
            hash = last.hash(),
            slot = u64::from(last.slot_or_default()),
            db_system_name = "rocksdb".to_string(),
            db_operation_name = "delete".to_string(),
            db_collection_name = "chain".to_string()
        );
        let _guard = _span.enter();

        let fork_slot = u64::from(fork_point.slot_or_default()).to_be_bytes();
        let fork_key = [&CHAIN_PREFIX[..], &fork_slot[..]].concat();

        let slot = (u64::from(fork_point.slot_or_default()) + 1).to_be_bytes();
        let mut opts = ReadOptions::default();
        opts.set_iterate_range(PrefixRange(&CHAIN_PREFIX[..]));
        let starting_point = [&CHAIN_PREFIX[..], &slot[..]].concat();
        let mode = IteratorMode::From(starting_point.as_slice(), rocksdb::Direction::Forward);

        self.with_transaction(|tx| {
            // Validate the fork point *inside* the transaction using `get_for_update`, so any
            // concurrent writer that deletes or overwrites the fork-point chain entry causes
            // this transaction to conflict on commit rather than silently succeeding against
            // stale state.
            let existing =
                tx.get_for_update(&fork_key, true).map_err(|e| StoreError::ReadError { error: e.to_string() })?;
            let matches = existing
                .as_ref()
                .map(|bytes| bytes.len() == HEADER && bytes.as_slice() == fork_point.hash().as_ref())
                .unwrap_or(false);
            if !matches {
                return Err(StoreError::ReadError {
                    error: format!(
                        "Cannot switch to a fork from point {:?} as it does not exist on the best chain",
                        fork_point
                    ),
                });
            }

            let keys_to_delete: Vec<_> = tx
                .iterator_opt(mode, opts)
                .map(|kv| kv.map(|(key, _)| key).map_err(|e| StoreError::ReadError { error: e.to_string() }))
                .collect::<Result<_, _>>()?;

            for key in keys_to_delete {
                tx.delete(key).map_err(|e| StoreError::WriteError { error: e.to_string() })?;
            }

            for point in forward_points.iter() {
                let slot = u64::from(point.slot_or_default()).to_be_bytes();
                tx.put([&CHAIN_PREFIX[..], &slot[..]].concat(), point.hash().as_ref())
                    .map_err(|e| StoreError::WriteError { error: e.to_string() })?;
            }

            tx.put(BEST_CHAIN_PREFIX, forward_points.last().hash().as_ref())
                .map_err(|e| StoreError::WriteError { error: e.to_string() })?;

            Ok(())
        })
    }

    fn roll_forward_chain(&self, point: &Point) -> Result<(), StoreError> {
        let _span = trace_span!(
            amaru_observability::amaru::stores::consensus::ROLL_FORWARD_CHAIN,
            hash = point.hash(),
            slot = u64::from(point.slot_or_default()),
            db_system_name = "rocksdb".to_string(),
            db_operation_name = "put".to_string(),
            db_collection_name = "chain".to_string()
        );
        let _guard = _span.enter();

        self.with_transaction(|tx| {
            let slot = u64::from(point.slot_or_default()).to_be_bytes();
            tx.put([&CHAIN_PREFIX[..], &slot[..]].concat(), point.hash().as_ref())
                .map_err(|e| StoreError::WriteError { error: e.to_string() })?;
            tx.put(BEST_CHAIN_PREFIX, point.hash().as_ref())
                .map_err(|e| StoreError::WriteError { error: e.to_string() })?;
            Ok(())
        })
    }
}

#[cfg(test)]
pub mod test {
    use std::{collections::BTreeMap, fs, io, path::Path, sync::Arc};

    use amaru_kernel::{
        BlockHeader, BlockHeight, Nonce, ORIGIN_HASH, Point, Slot, any_header_hash, any_header_with_parent,
        any_headers_chain, make_header,
        size::HEADER,
        utils::tests::{random_bytes, run_strategy},
    };
    use amaru_ouroboros_traits::{
        ChainStore, DiagnosticChainStore, MissingBlocks, NextBestChainHeader, ReadOnlyChainStore,
        RollbackPointSearchResult, in_memory_consensus_store::InMemConsensusStore,
    };
    use rocksdb::Direction;

    use super::*;
    use crate::rocksdb::consensus::{migration::migrate_db_path, util::CHAIN_DB_VERSION};

    #[test]
    fn both_rw_and_ro_can_be_open_on_same_dir() {
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path();
        let _rw_store = initialise_test_rw_store(path);
        if let Err(e) = initialise_test_ro_store(path) {
            panic!("failed to re-open DB in read-only mode: {}", e);
        }
    }

    #[test]
    fn rocksdb_chain_store_can_get_header_it_puts() {
        with_db(|db| {
            let header = BlockHeader::from(make_header(1, 0, None));
            db.store_header(&header).unwrap();
            let header2 = db.load_header(&header.hash()).unwrap();
            assert_eq!(header, header2);
        })
    }

    #[test]
    fn rocksdb_chain_store_can_get_block_it_puts() {
        with_db(|db| {
            let hash: HeaderHash = random_bytes(32).as_slice().into();
            let block = RawBlock::from(&*vec![1; 64]);

            db.store_block(&hash, &block).unwrap();
            let block2 = db.load_block(&hash).unwrap();
            assert_eq!(Some(block), block2);
        })
    }

    #[test]
    fn rocksdb_chain_store_returns_not_found_for_nonexistent_block() {
        with_db(|db| {
            let nonexistent_hash: HeaderHash = random_bytes(HEADER).as_slice().into();
            let result = db.load_block(&nonexistent_hash).unwrap();

            assert_eq!(result, None);
        });
    }

    #[test]
    fn best_chain_hash_when_store_is_empty() {
        with_db(|db| assert_eq!(db.get_best_chain_hash(), ORIGIN_HASH))
    }

    #[test]
    fn store_best_chain_hash() {
        with_db(|db| {
            let best_chain = run_strategy(any_header_hash());
            db.set_best_chain_hash(&best_chain).unwrap();
            assert_eq!(db.get_best_chain_hash(), best_chain);
        })
    }

    #[test]
    fn anchor_hash_when_store_is_empty() {
        with_db(|db| {
            assert_eq!(db.get_anchor_hash(), ORIGIN_HASH);
        })
    }

    #[test]
    fn store_anchor_hash() {
        with_db(|db| {
            let anchor = run_strategy(any_header_hash());
            db.set_anchor_hash(&anchor).unwrap();
            assert_eq!(db.get_anchor_hash(), anchor);
        })
    }

    #[test]
    fn store_parent_children_relationship_for_header() {
        with_db(|db| {
            // h0 -> h1 -> h2
            //      \
            //       -> h3
            let mut chain = run_strategy(any_headers_chain(3));
            let h3 = run_strategy(any_header_with_parent(chain[1].hash()));
            chain.push(h3.clone());

            for header in &chain {
                db.store_header(header).unwrap();
            }

            let mut children = db.get_children(&chain[1].hash());
            children.sort();
            let mut expected = vec![chain[2].hash(), h3.hash()];
            expected.sort();
            assert_eq!(children, expected);
        })
    }

    #[test]
    fn load_all_headers() {
        with_db_path(|(db, path)| {
            let mut headers: Vec<BlockHeader> = vec![];
            for i in 0..10usize {
                let parent = if i == 0 { None } else { Some(headers[i - 1].hash()) };
                let header = make_header(i as u64, i as u64 * 10, parent).into();
                db.store_header(&header).unwrap();
                headers.push(header);
            }
            headers.sort();

            let db = initialise_test_ro_store(path).unwrap();

            let mut result: Vec<BlockHeader> = db.load_headers().collect();
            result.sort();
            assert_eq!(result, headers);
        })
    }

    #[test]
    fn load_parents_children() {
        with_db_path(|(db, path)| {
            // h0 -> h1 -> h2
            //      \
            //       -> h3 -> h4
            let mut chain = run_strategy(any_headers_chain(3));
            let h3 = run_strategy(any_header_with_parent(chain[1].hash()));
            chain.push(h3.clone());
            let h4 = run_strategy(any_header_with_parent(h3.hash()));
            chain.push(h4);

            let mut expected = BTreeMap::new();

            for header in &chain {
                if let Some(parent) = header.parent() {
                    expected.entry(parent).or_insert_with(Vec::new).push(header.hash());
                }
                db.store_header(header).unwrap();
            }

            let db = initialise_test_ro_store(path).unwrap();

            let result = sort_entries(db.load_parents_children().collect::<Vec<_>>());
            let expected = sort_entries(expected.into_iter().collect::<Vec<_>>());
            assert_eq!(result, expected);
        })
    }

    #[test]
    fn load_nonces() {
        with_db_path(|(db, path)| {
            let chain = run_strategy(any_headers_chain(3));
            let mut expected = BTreeMap::new();
            for header in &chain {
                let nonces = Nonces {
                    active: Nonce::from(random_bytes(32).as_slice()),
                    evolving: Nonce::from(random_bytes(32).as_slice()),
                    candidate: Nonce::from(random_bytes(32).as_slice()),
                    tail: header.parent().unwrap_or(ORIGIN_HASH),
                    epoch: Default::default(),
                };
                db.put_nonces(&header.hash(), &nonces).unwrap();
                expected.insert(header.hash(), nonces);
            }

            let db = initialise_test_ro_store(path).unwrap();

            let mut result = db.load_nonces().collect::<Vec<_>>();
            result.sort();
            let mut expected = expected.into_iter().collect::<Vec<_>>();
            expected.sort();
            assert_eq!(result, expected);
        })
    }

    #[test]
    fn load_blocks() {
        with_db_path(|(db, path)| {
            let chain = run_strategy(any_headers_chain(3));
            let mut expected = BTreeMap::new();
            for header in &chain {
                let block = RawBlock::from(random_bytes(32).as_slice());
                db.store_block(&header.hash(), &block).unwrap();
                expected.insert(header.hash(), block);
            }

            let db = initialise_test_ro_store(path).unwrap();

            let mut result = db.load_blocks().collect::<Vec<_>>();
            result.sort();
            let mut expected = expected.into_iter().collect::<Vec<_>>();
            expected.sort();
            assert_eq!(result, expected);
        })
    }

    #[test]
    fn retrieve_best_chain() {
        with_db(|db| {
            // create a chain and store it as the best chain
            // with its anchor and tip.
            let chain = run_strategy(any_headers_chain(15));
            for header in &chain {
                db.store_header(header).unwrap();
            }

            // set the chain anchor to the 5th header
            // and the tip to the last header
            db.set_anchor_hash(&chain[4].hash()).unwrap();
            db.set_best_chain_hash(&chain[14].hash()).unwrap();
            let result = db.retrieve_best_chain();
            assert_eq!(result, chain[4..].iter().map(|h| h.hash()).collect::<Vec<_>>());
        })
    }

    #[test]
    fn load_from_best_chain_root_header() {
        with_db(|store| {
            let chain = populate_db(store.clone());
            let root = run_strategy(any_header_with_parent(chain[0].hash()));

            store.roll_forward_chain(&root.point()).expect("should roll forward successfully");

            assert_eq!(store.load_from_best_chain(&root.point()), Some(root.hash()));
            assert_eq!(store.get_best_chain_hash(), root.hash());
        });
    }

    #[test]
    fn update_best_chain_to_block_slot_given_new_block_is_valid() {
        with_db(|store| {
            let chain = populate_db(store.clone());
            let new_tip = run_strategy(any_header_with_parent(chain[9].hash()));

            store.roll_forward_chain(&new_tip.point()).expect("should roll forward successfully");

            assert_eq!(store.load_from_best_chain(&new_tip.point()), Some(new_tip.hash()));
            assert_eq!(store.get_best_chain_hash(), new_tip.hash());
        });
    }

    #[test]
    fn switch_to_fork_switches_to_fork_and_updates_tip() {
        with_db(|store| {
            let headers = make_forked_headers();
            append_best_chain(store.clone(), headers.main());
            for header in [&headers.h2a, &headers.h3a] {
                store.store_header(header).unwrap();
            }

            store
                .switch_to_fork(
                    &headers.h1.point(),
                    &NonEmptyVec::try_from(vec![headers.h2a.point(), headers.h3a.point()]).unwrap(),
                )
                .expect("should replace the best chain successfully");

            assert_eq!(store.get_best_chain_hash(), headers.h3a.hash());
            assert_eq!(store.load_from_best_chain(&headers.h3.point()), None);
            assert_eq!(store.load_from_best_chain(&headers.h2a.point()), Some(headers.h2a.hash()));
            assert_eq!(store.load_from_best_chain(&headers.h3a.point()), Some(headers.h3a.hash()));
        });
    }

    #[test]
    fn switch_to_fork_raises_error_if_fork_point_is_not_on_best_chain() {
        with_db(|store| {
            let headers = make_forked_headers();
            append_best_chain(store.clone(), headers.main());
            for header in [&headers.h2a, &headers.h3a] {
                store.store_header(header).unwrap();
            }

            let result = store.switch_to_fork(&headers.h2a.point(), &NonEmptyVec::singleton(headers.h3a.point()));

            if result.is_ok() {
                panic!("expected test to fail");
            }
        });
    }

    #[test]
    fn switch_to_fork_preserves_state_when_fork_point_is_not_on_best_chain() {
        // Atomicity: a switch_to_fork call that fails its fork-point check must leave both
        // the chain index AND the best-tip pointer unchanged from pre-call state.
        with_db(|store| {
            let headers = make_forked_headers();
            append_best_chain(store.clone(), headers.main());
            for header in [&headers.h2a, &headers.h3a] {
                store.store_header(header).unwrap();
            }

            let best_chain_before = store.get_best_chain_hash();
            let chain_before = store.retrieve_best_chain();

            let result = store.switch_to_fork(&headers.h2a.point(), &NonEmptyVec::singleton(headers.h3a.point()));
            assert!(result.is_err(), "expected fork-point-not-on-chain error");

            assert_eq!(store.get_best_chain_hash(), best_chain_before, "best tip must not move");
            assert_eq!(store.retrieve_best_chain(), chain_before, "chain index must be unchanged");
            assert_eq!(store.load_from_best_chain(&headers.h3.point()), Some(headers.h3.hash()));
            assert_eq!(store.load_from_best_chain(&headers.h3a.point()), None);
        });
    }

    #[test]
    fn find_fork_point_returns_none_when_start_header_is_not_in_store() {
        with_db(|store| {
            let headers = make_forked_headers();
            append_best_chain(store.clone(), headers.main());
            store.set_anchor_hash(&headers.h0.hash()).unwrap();

            let absent = run_strategy(any_header_hash());
            assert_eq!(store.find_fork_point(absent), None);
        });
    }

    #[test]
    fn find_fork_point_handles_one_block_fork_off_non_tip() {
        // Best chain: h0 -> h1 -> h2 -> h3
        //                   \
        //                    -> h2a (start, single-block fork off h1)
        with_db(|store| {
            let headers = make_forked_headers();
            append_best_chain(store.clone(), headers.main());
            store.set_anchor_hash(&headers.h0.hash()).unwrap();
            store.store_header(&headers.h2a).unwrap();

            let result = store.find_fork_point(headers.h2a.hash());
            assert_eq!(result, Some((headers.h1.point(), NonEmptyVec::singleton(headers.h2a.point()))));
        });
    }

    #[test]
    fn find_missing_blocks_preserves_boundary_parent_invariant_under_truncation() {
        // For any non-empty return, missing[0].parent() == boundary, regardless of limit.
        with_db(|store| {
            // h0 -> ... -> h9, all headers stored, block present only for h0.
            let chain = populate_db(store.clone());
            let block = RawBlock::from(&*vec![1; 64]);
            store.store_block(&chain[0].hash(), &block).unwrap();

            for limit in 1..=9usize {
                let result = store.find_missing_blocks(chain[9].hash(), limit).unwrap();
                let range = result.expect("range exists when start header is in the store");
                let boundary = range.boundary();
                let first_missing = range.first().expect("non-empty missing list with block gap present");
                let first_missing_header =
                    store.load_header(&first_missing.hash()).expect("first missing header exists");
                assert_eq!(
                    first_missing_header.parent(),
                    Some(boundary.hash()),
                    "invariant broken at limit={}: missing[0].parent() != boundary",
                    limit,
                );
                assert!(
                    range.nb_missing_blocks() <= limit,
                    "truncation not respected at limit={}: got {}",
                    limit,
                    range.nb_missing_blocks(),
                );
            }
        });
    }

    #[test]
    fn next_best_chain_returns_successor_give_valid_point() {
        with_db(|store| {
            let chain = populate_db(store.clone());

            let result = store.next_best_chain(&chain[5].point()).expect("should find successor");

            assert_eq!(result, chain[6].point());
        });
    }

    #[test]
    fn next_best_chain_returns_first_point_on_chain_given_origin() {
        with_db(|store| {
            let chain = populate_db(store.clone());

            let result = store.next_best_chain(&Point::Origin).expect("should find successor");

            assert_eq!(result, chain[0].point());
        });
    }

    #[test]
    fn next_best_chain_returns_none_given_point_is_not_on_chain() {
        with_db(|store| {
            let _chain = populate_db(store.clone());
            let invalid_point = Point::Specific(100.into(), run_strategy(any_header_hash()));

            assert!(store.next_best_chain(&invalid_point).is_none());
        });
    }

    #[test]
    fn next_best_chain_returns_none_given_point_is_tip() {
        with_db(|store| {
            let _chain = populate_db(store.clone());
            let tip = store.get_best_chain_hash();
            let tip_header = store.load_header(&tip).unwrap();

            assert!(store.next_best_chain(&tip_header.point()).is_none());
        });
    }

    #[test]
    fn next_best_chain_header_rolls_forward_from_best_chain_pointer() {
        with_db(|store| {
            let chain = populate_db(store.clone());

            let result = store.next_best_chain_header(&chain[5].point()).unwrap();

            assert_eq!(result, NextBestChainHeader::RollForward { point: chain[6].point(), header: chain[6].clone() });
        });
    }

    #[test]
    fn next_best_chain_header_rolls_forward_from_origin() {
        with_db(|store| {
            let chain = populate_db(store.clone());
            let result = store.next_best_chain_header(&Point::Origin).unwrap();
            assert_eq!(result, NextBestChainHeader::RollForward { point: chain[0].point(), header: chain[0].clone() });
        });
    }

    #[test]
    fn next_best_chain_header_requests_rollback_for_non_best_chain_pointer() {
        with_db(|store| {
            let headers = make_forked_headers();
            append_best_chain(store.clone(), headers.main());
            for header in [&headers.h2a, &headers.h3a] {
                store.store_header(header).unwrap();
            }

            let result = store.next_best_chain_header(&headers.h3a.point()).unwrap();

            assert_eq!(result, NextBestChainHeader::NeedRollback);
        });
    }

    #[test]
    fn next_best_chain_header_reports_at_tip() {
        with_db(|store| {
            let chain = populate_db(store.clone());

            let result = store.next_best_chain_header(&chain[9].point()).unwrap();

            assert_eq!(result, NextBestChainHeader::AtTip);
        });
    }

    #[test]
    fn find_anchor_at_height_returns_first_header_at_or_above_target() {
        with_db(|store| {
            // populate_db sets the anchor to chain[0] (block_height = 1) and best chain to chain[9].
            let chain = populate_db(store.clone());

            let result = store.find_anchor_at_height(BlockHeight::from(5));
            assert_eq!(result, Some(chain[4].hash()));
        });
    }

    #[test]
    fn find_anchor_at_height_returns_none_when_target_at_or_below_current_anchor() {
        with_db(|store| {
            // Anchor is at chain[0] (block_height = 1).
            let _chain = populate_db(store.clone());

            assert!(store.find_anchor_at_height(BlockHeight::from(0)).is_none());
            assert!(store.find_anchor_at_height(BlockHeight::from(1)).is_none());
        });
    }

    #[test]
    fn find_anchor_at_height_returns_none_when_target_beyond_best_chain() {
        with_db(|store| {
            // Best chain tip is chain[9] (block_height = 10).
            let _chain = populate_db(store.clone());

            assert!(store.find_anchor_at_height(BlockHeight::from(100)).is_none());
        });
    }

    #[test]
    fn find_anchor_at_height_walks_from_origin_when_anchor_is_origin() {
        with_db(|store| {
            // Do not set an anchor; leave it at ORIGIN. Only roll forward the best chain.
            let chain = run_strategy(any_headers_chain(10));
            for header in chain.iter() {
                store.store_header(header).unwrap();
                store.roll_forward_chain(&header.point()).unwrap();
            }
            assert_eq!(store.get_anchor_hash(), ORIGIN_HASH);

            let result = store.find_anchor_at_height(BlockHeight::from(3));
            assert_eq!(result, Some(chain[2].hash()));
        });
    }

    #[test]
    fn unvalidated_ancestor_hashes_returns_missing_validity_segment_in_chain_order() {
        with_db(|store| {
            // h0 -> h1(valid) -> h2(?) -> h3(start)
            //        \
            //         -> h2a -> h3a
            let headers = make_forked_headers();
            for header in headers.all() {
                store.store_header(header).unwrap();
            }
            store.set_block_valid(&headers.h1.hash(), true).unwrap();

            let result = store.unvalidated_ancestor_hashes(headers.h3.hash());
            assert_eq!(result, (vec![headers.h2.hash(), headers.h3.hash()], true));
        });
    }

    #[test]
    fn unvalidated_ancestor_hashes_marks_chain_invalid_when_it_hits_invalid_ancestor() {
        with_db(|store| {
            // h0 -> h1(invalid) -> h2(?) -> h3(start)
            //          \
            //           -> h2a -> h3a
            let headers = make_forked_headers();
            for header in headers.all() {
                store.store_header(header).unwrap();
            }
            store.set_block_valid(&headers.h1.hash(), false).unwrap();

            let result = store.unvalidated_ancestor_hashes(headers.h3.hash());
            assert_eq!(result, (vec![headers.h2.hash(), headers.h3.hash()], false));
        });
    }

    #[test]
    fn find_fork_point_returns_best_chain_intersection_and_forward_path() {
        with_db(|store| {
            // Best chain: h0 -> h1 -> h2 -> h3
            //                   \
            // fork:              -> h2a -> h3a(start)
            let headers = make_forked_headers();
            append_best_chain(store.clone(), headers.main());
            store.set_anchor_hash(&headers.h0.hash()).unwrap();
            for header in [&headers.h2a, &headers.h3a] {
                store.store_header(header).unwrap();
            }

            let result = store.find_fork_point(headers.h3a.hash());
            assert_eq!(
                result,
                Some((headers.h1.point(), NonEmptyVec::new(headers.h2a.point(), vec![headers.h3a.point()])))
            );
        });
    }

    #[test]
    fn find_fork_point_returns_none_when_start_is_already_on_best_chain() {
        with_db(|store| {
            let headers = make_forked_headers();
            append_best_chain(store.clone(), headers.main());
            store.set_anchor_hash(&headers.h0.hash()).unwrap();

            let result = store.find_fork_point(headers.h3.hash());
            assert_eq!(result, None);
        });
    }

    #[test]
    fn find_rollback_point_returns_closest_valid_best_chain_ancestor() {
        with_db(|store| {
            let headers = make_forked_headers();
            append_best_chain(store.clone(), headers.main());
            store.set_anchor_hash(&headers.h0.hash()).unwrap();
            store.set_block_valid(&headers.h1.hash(), true).unwrap();
            for header in [&headers.h2a, &headers.h3a] {
                store.store_header(header).unwrap();
            }

            let result = store.find_rollback_point(headers.h3a.hash(), headers.h0.point());

            assert_eq!(
                result,
                RollbackPointSearchResult::Found {
                    point: headers.h1.point(),
                    forward_points: vec![headers.h2a.point(), headers.h3a.point()],
                    chosen_because_contains: true,
                }
            );
        });
    }

    #[test]
    fn find_rollback_point_does_not_use_best_chain_ancestor_with_unknown_validity() {
        with_db(|store| {
            let headers = make_forked_headers();
            append_best_chain(store.clone(), headers.main());
            store.set_anchor_hash(&headers.h0.hash()).unwrap();
            store.set_block_valid(&headers.h0.hash(), true).unwrap();
            for header in [&headers.h2a, &headers.h3a] {
                store.store_header(header).unwrap();
            }

            let result = store.find_rollback_point(headers.h3a.hash(), Point::Origin);

            assert_eq!(
                result,
                RollbackPointSearchResult::Found {
                    point: headers.h0.point(),
                    forward_points: vec![headers.h1.point(), headers.h2a.point(), headers.h3a.point()],
                    chosen_because_contains: true,
                }
            );
        });
    }

    #[test]
    fn find_common_ancestor_returns_shared_point_between_forks() {
        with_db(|store| {
            // h0 -> h1 -> h2 -> h3
            //        \
            //         -> h2a -> h3a
            // common_ancestor(h3, h3a) = h1
            let headers = make_forked_headers();
            for header in headers.all() {
                store.store_header(header).unwrap();
            }

            let result = store.find_common_ancestor(headers.h3.hash(), headers.h3a.hash());
            assert_eq!(result, Some(headers.h1.point()));
        });
    }

    #[test]
    fn sample_ancestor_points_returns_exponential_walk_back_from_best_tip() {
        with_db(|store| {
            // Best chain:
            // h0 -> h1 -> h2 -> h3 -> h4 -> h5 -> h6 -> h7 -> h8 -> h9 (tip)
            // Samples:
            // h9, h8, h7, h5, h1, h0
            let chain = populate_db(store.clone());

            let result = store.sample_ancestor_points();

            assert_eq!(
                result,
                vec![
                    chain[9].point(),
                    chain[8].point(),
                    chain[7].point(),
                    chain[5].point(),
                    chain[1].point(),
                    chain[0].point(),
                ]
            );
        });
    }

    #[test]
    fn test_intersect_points_includes_best_point_and_are_spaced_with_a_factor_2() {
        with_db(|store| {
            let mut parent = None;
            for slot in 0..=100 {
                let header = BlockHeader::from(make_header(slot + 1, slot, parent));
                store.store_header(&header).unwrap();
                store.roll_forward_chain(&header.point()).unwrap();
                parent = Some(header.hash());
            }

            let result = store.sample_ancestor_points();
            let slots = result.iter().map(|point| u64::from(point.slot_or_default())).collect::<Vec<_>>();

            assert_eq!(slots, vec![100, 99, 98, 96, 92, 84, 68, 36, 0]);
        });
    }

    #[test]
    fn find_missing_blocks_returns_path_from_nearest_available_block_to_tip() {
        with_db(|store| {
            // Best chain:
            // h0 -> h1 -> h2 -> h3 -> h4 -> h5 -> h6 -> h7 -> h8 -> h9 (tip)
            //                                     *
            //                                 block present
            // Missing path to fetch from h9:
            // h6 -> h7 -> h8 -> h9
            let chain = populate_db(store.clone());
            let block = RawBlock::from(&*vec![1; 64]);
            store.store_block(&chain[6].hash(), &block).unwrap();

            let result = store.find_missing_blocks(chain[9].hash(), 10).unwrap();

            assert_eq!(
                result,
                Some(MissingBlocks::new(chain[6].point(), vec![chain[7].point(), chain[8].point(), chain[9].point()],))
            );
        });
    }

    #[test]
    fn find_missing_blocks_returns_none_when_tip_is_not_found() {
        with_db(|store| {
            let missing_tip = run_strategy(any_header_hash());
            let result = store.find_missing_blocks(missing_tip, 10).unwrap();
            assert_eq!(result, None);
        });
    }

    #[test]
    fn find_missing_blocks_returns_boundary_only_when_tip_block_exists() {
        with_db(|store| {
            // Best chain:
            // h0 -> h1 -> h2 -> h3 -> h4 -> h5 -> h6 -> h7 -> h8 -> h9 (tip)
            //                                                   *
            //                                               block present
            let chain = populate_db(store.clone());
            let block = RawBlock::from(&*vec![1; 64]);
            store.store_block(&chain[9].hash(), &block).unwrap();

            let result = store.find_missing_blocks(chain[9].hash(), 10).unwrap();
            assert_eq!(result, Some(MissingBlocks::new(chain[9].point(), vec![])));
        });
    }

    #[test]
    fn read_snapshot_keeps_original_best_chain_view_after_store_changes() {
        with_read_db(
            |store| {
                populate_db(store);
            },
            |store, snapshot| {
                let best_chain_hash = snapshot.get_best_chain_hash();
                let best_chain = snapshot.retrieve_best_chain();
                let tip = snapshot.load_header(&best_chain_hash).expect("tip should exist in snapshot");
                let next_slot = u64::from(tip.slot()) + 1;
                let new_tip = BlockHeader::from(make_header(next_slot, next_slot, Some(tip.hash())));

                store.store_header(&new_tip).expect("should store header successfully");
                store.roll_forward_chain(&new_tip.point()).expect("should roll forward successfully");
                assert_eq!(snapshot.get_best_chain_hash(), best_chain_hash);
                assert_eq!(snapshot.retrieve_best_chain(), best_chain);
                assert_eq!(snapshot.load_from_best_chain(&new_tip.point()), None);
                assert!(snapshot.next_best_chain(&tip.point()).is_none());
            },
        );
    }

    #[test]
    fn read_snapshot_exposes_direct_read_operations() {
        let headers = make_forked_headers();
        let nonces = Nonces {
            active: Nonce::from(random_bytes(32).as_slice()),
            evolving: Nonce::from(random_bytes(32).as_slice()),
            candidate: Nonce::from(random_bytes(32).as_slice()),
            tail: headers.h1.hash(),
            epoch: Default::default(),
        };
        let block = RawBlock::from(&*vec![1; 64]);

        with_read_db(
            {
                let headers = headers.clone();
                let nonces = nonces.clone();
                let block = block.clone();
                move |store| {
                    append_best_chain(store.clone(), headers.main());
                    for header in [&headers.h2a, &headers.h3a] {
                        store.store_header(header).unwrap();
                    }
                    store.set_anchor_hash(&headers.h0.hash()).unwrap();
                    store.put_nonces(&headers.h2.hash(), &nonces).unwrap();
                    store.store_block(&headers.h3.hash(), &block).unwrap();
                    store.set_block_valid(&headers.h2.hash(), true).unwrap();
                }
            },
            {
                let headers = headers.clone();
                let nonces = nonces.clone();
                let block = block.clone();
                move |_store, snapshot| {
                    let mut children = snapshot.get_children(&headers.h1.hash());
                    children.sort();

                    assert_eq!(snapshot.load_header(&headers.h2.hash()), Some(headers.h2.clone()));
                    assert_eq!(
                        snapshot.load_header_with_validity(&headers.h2.hash()),
                        Some((headers.h2.clone(), Some(true)))
                    );
                    assert_eq!(children, vec![headers.h2.hash(), headers.h2a.hash()]);
                    assert_eq!(snapshot.get_anchor_hash(), headers.h0.hash());
                    assert_eq!(snapshot.get_best_chain_hash(), headers.h3.hash());
                    assert_eq!(snapshot.get_nonces(&headers.h2.hash()), Some(nonces.clone()));
                    assert_eq!(snapshot.load_block(&headers.h3.hash()).unwrap(), Some(block.clone()));
                    assert!(snapshot.has_header(&headers.h3a.hash()));
                    assert!(!snapshot.has_header(&run_strategy(any_header_hash())));
                }
            },
        );
    }

    #[test]
    fn read_snapshot_supports_best_chain_traversal() {
        let chain = make_linear_headers(10);

        with_read_db(
            {
                let chain = chain.clone();
                move |store| {
                    store.set_anchor_hash(&chain[0].hash()).unwrap();
                    append_best_chain(store.clone(), &chain);
                }
            },
            {
                let chain = chain.clone();
                move |_store, snapshot| {
                    let invalid_point = Point::Specific(100.into(), run_strategy(any_header_hash()));

                    assert_eq!(snapshot.retrieve_best_chain(), chain.iter().map(BlockHeader::hash).collect::<Vec<_>>());
                    assert_eq!(snapshot.load_from_best_chain(&chain[0].point()), Some(chain[0].hash()));
                    assert_eq!(snapshot.load_from_best_chain(&invalid_point), None);
                    assert_eq!(snapshot.next_best_chain(&Point::Origin), Some(chain[0].point()));
                    assert_eq!(snapshot.next_best_chain(&chain[5].point()), Some(chain[6].point()));
                    assert_eq!(snapshot.next_best_chain(&chain[9].point()), None);
                }
            },
        );
    }

    #[test]
    fn read_snapshot_supports_find_intersect_point_queries() {
        let chain = make_linear_headers(10);

        with_read_db(
            {
                let chain = chain.clone();
                move |store| {
                    append_best_chain(store.clone(), &chain);
                    store.set_anchor_hash(&chain[5].hash()).unwrap();
                }
            },
            {
                let chain = chain.clone();
                move |store, _snapshot| {
                    let unknown = Point::Specific(Slot::from(999), Hash::new([0xff; HEADER]));

                    // intersect one point
                    assert_eq!(store.find_intersect_point(vec![chain[5].point()]), Some(chain[5].point()));
                    // intersect the most recent point
                    assert_eq!(
                        store.find_intersect_point(vec![chain[3].point(), chain[7].point()]),
                        Some(chain[7].point())
                    );
                    // intersect the most recent point
                    assert_eq!(store.find_intersect_point(vec![chain[2].point()]), Some(chain[2].point()));
                    // intersect with no points
                    assert_eq!(store.find_intersect_point(vec![]), None);
                    // intersect with an unknown pointThe thing is not comparable.
                    assert_eq!(store.find_intersect_point(vec![unknown]), None);
                }
            },
        );
    }

    #[test]
    fn find_intersect_point_returns_origin_when_best_chain_is_non_empty() {
        with_db(|store| {
            let _chain = populate_db(store.clone());

            assert_eq!(store.find_intersect_point(vec![Point::Origin]), Some(Point::Origin));
        });
    }

    #[test]
    fn read_snapshot_supports_ancestor_fork_and_sampling_queries() {
        let headers = make_forked_headers();
        let chain = make_linear_headers(10);

        with_read_db(
            {
                let headers = headers.clone();
                let chain = chain.clone();
                move |store| {
                    append_best_chain(store.clone(), headers.main());
                    for header in [&headers.h2a, &headers.h3a] {
                        store.store_header(header).unwrap();
                    }
                    store.set_anchor_hash(&headers.h0.hash()).unwrap();
                    store.set_block_valid(&headers.h1.hash(), true).unwrap();

                    append_best_chain(store.clone(), &chain[4..]);
                }
            },
            {
                let headers = headers.clone();
                let chain = chain.clone();
                move |store, _snapshot| {
                    assert_eq!(
                        store.unvalidated_ancestor_hashes(headers.h3.hash()),
                        (vec![headers.h2.hash(), headers.h3.hash()], true)
                    );
                    assert_eq!(
                        store.find_fork_point(headers.h3a.hash()),
                        Some((headers.h1.point(), NonEmptyVec::new(headers.h2a.point(), vec![headers.h3a.point()])))
                    );
                    assert_eq!(
                        store.find_common_ancestor(headers.h3.hash(), headers.h3a.hash()),
                        Some(headers.h1.point())
                    );
                    assert_eq!(
                        store.sample_ancestor_points(),
                        vec![
                            chain[9].point(),
                            chain[8].point(),
                            chain[7].point(),
                            chain[5].point(),
                            chain[1].point(),
                            chain[0].point(),
                        ]
                    );
                }
            },
        );
    }

    #[test]
    fn read_snapshot_supports_missing_block_queries() {
        let chain = make_linear_headers(10);
        let block = RawBlock::from(&*vec![1; 64]);

        with_read_db(
            {
                let chain = chain.clone();
                let block = block.clone();
                move |store| {
                    store.set_anchor_hash(&chain[0].hash()).unwrap();
                    append_best_chain(store.clone(), &chain);
                    store.store_block(&chain[6].hash(), &block).unwrap();
                }
            },
            {
                let chain = chain.clone();
                move |store, _snapshot| {
                    let missing_tip = run_strategy(any_header_hash());

                    assert_eq!(
                        store.find_missing_blocks(chain[9].hash(), 10).unwrap(),
                        Some(MissingBlocks::new(
                            chain[6].point(),
                            vec![chain[7].point(), chain[8].point(), chain[9].point()],
                        ))
                    );
                    assert_eq!(store.find_missing_blocks(missing_tip, 10).unwrap(), None);
                }
            },
        );
    }

    // MIGRATIONS

    #[test]
    fn fails_to_open_rw_db_if_it_does_not_exist() {
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path();
        let basedir = init_dir(path);
        let config = RocksDbConfig::new(basedir);

        let result = RocksDBStore::open(&config);
        match result {
            Err(StoreError::OpenError { .. }) => (), // OK
            Err(e) => panic!("Expected OpenError error, got: {:?}", e),
            _other => panic!("Expected failure to open RocksDBStore but it succeeded"),
        }
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn fails_to_open_rw_db_if_it_exists_with_wrong_version() {
        let tempdir = tempfile::tempdir().unwrap();
        let target = tempdir.path();
        let config = RocksDbConfig::new(target.to_path_buf());
        let source = PathBuf::from("sample-chain-db/v0");

        copy_recursively(source, target).unwrap();

        let result = RocksDBStore::open(&config);
        match result {
            Err(StoreError::IncompatibleChainStoreVersions { stored, current }) => {
                assert_eq!(stored, 0);
                assert_eq!(current, CHAIN_DB_VERSION);
            }
            Err(e) => panic!("Expected OpenError error, got: {:?}", e),
            _other => panic!("Expected failure to open RocksDBStore but it succeeded"),
        }
    }

    #[test]
    fn raises_an_error_when_creating_a_database_given_directory_is_non_empty() {
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path();
        let basedir = init_dir(path);

        fs::write(basedir.join("some_file.txt"), b"some data").unwrap();
        let result = RocksDBStore::create(RocksDbConfig::new(basedir));

        match result {
            Err(StoreError::OpenError { error: _ }) => {
                // expected
            }
            Err(e) => panic!("Expected OpenError, got: {:?}", e),
            _other => panic!("Expected failure to create DB but it succeeded"),
        };
    }

    #[test]
    fn creates_a_database_with_current_version_given_directory_is_empty() {
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path();
        let basedir = init_dir(path);

        let store = RocksDBStore::create(RocksDbConfig::new(basedir)).expect("should create DB successfully");
        let version = get_version(&store.db).expect("should read version successfully");

        assert_eq!(version, CHAIN_DB_VERSION);
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn can_convert_v0_sample_db_to_v1() {
        let tempdir = tempfile::tempdir().unwrap();
        let target = tempdir.path();
        let config = RocksDbConfig::new(target.to_path_buf());
        let source = PathBuf::from("sample-chain-db/v0");

        copy_recursively(source, target).unwrap();

        let (_, db) = open_db(&config).expect("cannot open sample v0 DB");
        migrate_to_v1(&db).expect("Migration should succeed");

        let header: Option<BlockHeader> = db
            .get_pinned([&HEADER_PREFIX[..], hex::decode(SAMPLE_HASH).unwrap().as_slice()].concat())
            .ok()
            .and_then(|bytes| from_cbor(bytes?.as_ref()));

        assert!(header.is_some(), "Sample data should be preserved");
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn can_convert_v1_sample_db_to_v2() {
        use std::str::FromStr;

        let tempdir = tempfile::tempdir().unwrap();
        let target = tempdir.path();
        let config = RocksDbConfig::new(target.to_path_buf());
        let source = PathBuf::from("sample-chain-db/v1");

        copy_recursively(source, target).unwrap();

        let result = migrate_db_path(target).expect("Migration should succeed");

        let db = RocksDBStore::open(&config).expect("DB should successfully be opened as it's been migrated");
        assert_eq!((1, 3), result);
        let header: Option<HeaderHash> = <RocksDBStore as ReadOnlyChainStore<BlockHeader>>::load_from_best_chain(
            &db,
            &Point::Specific(5.into(), Hash::from_str(SAMPLE_HASH).unwrap()),
        );
        assert!(header.is_some(), "Sample data should be preserved");
    }

    #[test]
    fn migrate_db_fails_given_directory_does_not_exist() {
        let tempdir = tempfile::tempdir().unwrap();
        let target = tempdir.path();

        let result = migrate_db_path(target);

        match result {
            Err(StoreError::OpenError { error: _ }) => {
                // expected
            }
            Err(e) => panic!("Expected OpenError, got: {:?}", e),
            _other => panic!("Expected failure to migrate DB but it succeeded"),
        }
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn open_or_create_succeeds_given_directory_exists() {
        let tempdir = tempfile::tempdir().unwrap();
        let target = tempdir.path();
        let config = RocksDbConfig::new(target.to_path_buf());
        let source = PathBuf::from("sample-chain-db/v0");

        copy_recursively(source, target).unwrap();

        let store = RocksDBStore::open_and_migrate(&config).expect("should create DB successfully");
        let version = get_version(&store.db).expect("should read version successfully");

        assert_eq!(version, CHAIN_DB_VERSION);
    }

    #[test]
    fn iterator_over_chain() {
        let tempdir = tempfile::tempdir().unwrap();
        let target = tempdir.path();
        let config = RocksDbConfig::new(target.to_path_buf());
        let (_, db) = open_or_create_db(&config).expect("should open DB successfully");

        // populate DB
        for slot in 1..10 {
            let prefix = [&CHAIN_PREFIX[..], &(slot as u64).to_be_bytes()[..]].concat();
            let header_hash = run_strategy(any_header_hash());
            db.put(&prefix, header_hash).expect("should put data successfully");
        }
        // iterate over chain from 4 to 8
        let slot4 = 4u64.to_be_bytes();
        let slot6 = 6u64.to_be_bytes();
        let slot7 = 7u64.to_be_bytes();
        let slot8 = 8u64.to_be_bytes();
        let slot9 = 9u64.to_be_bytes();
        let slot10 = 10u64.to_be_bytes();
        let prefix = [&CHAIN_PREFIX[..], &slot4].concat();

        let mut readopts = ReadOptions::default();
        readopts.set_iterate_upper_bound([&CHAIN_PREFIX[..], &slot10[..]].concat());
        let mut iter = db.iterator_opt(IteratorMode::From(&prefix, Direction::Forward), readopts);
        let mut count = 0;
        while let Some(Ok((_, v))) = iter.next()
            && count < 3
        {
            let _header_hash: HeaderHash = Hash::from(v.as_ref());
            count += 1;
        }

        // we can delete keys the iterator has seen and not seen
        db.delete([&CHAIN_PREFIX[..], &slot6].concat()).expect("should delete data successfully");
        db.delete([&CHAIN_PREFIX[..], &slot7].concat()).expect("should delete data successfully");

        // iterator continues from where it left off, skipping deleted keys
        assert_eq!(*(iter.next().unwrap().unwrap().0), [&CHAIN_PREFIX[..], &slot8].concat());
        assert_eq!(*(iter.next().unwrap().unwrap().0), [&CHAIN_PREFIX[..], &slot9].concat());
    }

    // HELPERS

    const SAMPLE_HASH: &str = "4b1f95026700f5b3df8432b3f93b023f3cbdf13c85704e0f71b0089e6e81c947";

    #[derive(Clone)]
    struct ForkedHeaders {
        h0: BlockHeader,
        h1: BlockHeader,
        h2: BlockHeader,
        h3: BlockHeader,
        h2a: BlockHeader,
        h3a: BlockHeader,
    }

    impl ForkedHeaders {
        fn main(&self) -> [&BlockHeader; 4] {
            [&self.h0, &self.h1, &self.h2, &self.h3]
        }

        fn all(&self) -> [&BlockHeader; 6] {
            [&self.h0, &self.h1, &self.h2, &self.h3, &self.h2a, &self.h3a]
        }
    }

    fn make_forked_headers() -> ForkedHeaders {
        let h0 = BlockHeader::from(make_header(1, 1, None));
        let h1 = BlockHeader::from(make_header(2, 2, Some(h0.hash())));
        let h2 = BlockHeader::from(make_header(3, 3, Some(h1.hash())));
        let h3 = BlockHeader::from(make_header(4, 4, Some(h2.hash())));
        let h2a = BlockHeader::from(make_header(3, 10, Some(h1.hash())));
        let h3a = BlockHeader::from(make_header(4, 11, Some(h2a.hash())));

        ForkedHeaders { h0, h1, h2, h3, h2a, h3a }
    }

    fn make_linear_headers(len: usize) -> Vec<BlockHeader> {
        let mut headers = Vec::with_capacity(len);
        for i in 0..len {
            let parent = headers.last().map(BlockHeader::hash);
            headers.push(BlockHeader::from(make_header((i + 1) as u64, (i + 1) as u64, parent)));
        }
        headers
    }

    fn append_best_chain<'a>(
        store: Arc<dyn ChainStore<BlockHeader>>,
        headers: impl IntoIterator<Item = &'a BlockHeader>,
    ) {
        for header in headers {
            store.store_header(header).unwrap();
            store.roll_forward_chain(&header.point()).unwrap();
        }
    }

    fn populate_db(store: Arc<dyn ChainStore<BlockHeader>>) -> Vec<BlockHeader> {
        let chain = run_strategy(any_headers_chain(10));

        // Set the anchor to the first header in the chain
        store.set_anchor_hash(&chain[0].hash()).expect("should set anchor hash successfully");

        for header in chain.iter() {
            store.roll_forward_chain(&header.point()).expect("should roll forward successfully");
            store.store_header(header).expect("should store header successfully");
        }
        chain
    }

    pub fn initialise_test_rw_store(path: &std::path::Path) -> RocksDBStore {
        let basedir = init_dir(path);
        let config = RocksDbConfig::new(basedir);

        RocksDBStore::create(config).expect("fail to initialise RocksDB")
    }

    pub fn initialise_test_ro_store(path: &std::path::Path) -> Result<RocksDBStore<DB>, StoreError> {
        let basedir = init_dir(path);
        RocksDBStore::open_for_readonly(&RocksDbConfig::new(basedir))
    }

    fn init_dir(path: &std::path::Path) -> PathBuf {
        let basedir = path.join("rocksdb_chain_store");
        use std::fs::create_dir_all;
        create_dir_all(&basedir).expect("fail to create test dir");
        basedir
    }

    // creates a sample db at the given path, populating with some data
    // this function exists to make it easy to test DB migration:
    //
    // 1. create a sample DB with the old version
    // 2. run migration code
    // 3. check that the data is still there and correct
    #[expect(dead_code)]
    fn create_sample_db(path: &Path) {
        let db = initialise_test_rw_store(path);
        let chain = run_strategy(any_headers_chain(10));
        for header in &chain {
            let block = RawBlock::from(random_bytes(32).as_slice());
            db.store_header(header).unwrap();
            <RocksDBStore as ChainStore<BlockHeader>>::store_block(&db, &header.hash(), &block).unwrap();
            let nonces = Nonces {
                active: Nonce::from(random_bytes(32).as_slice()),
                evolving: Nonce::from(random_bytes(32).as_slice()),
                candidate: Nonce::from(random_bytes(32).as_slice()),
                tail: header.parent().unwrap_or(ORIGIN_HASH),
                epoch: Default::default(),
            };
            <RocksDBStore as ChainStore<BlockHeader>>::put_nonces(&db, &header.hash(), &nonces).unwrap();
        }
        <RocksDBStore as ChainStore<BlockHeader>>::set_anchor_hash(&db, &chain[1].hash()).unwrap();
        <RocksDBStore as ChainStore<BlockHeader>>::set_best_chain_hash(&db, &chain[9].hash()).unwrap();
    }

    fn with_db(f: impl Fn(Arc<dyn ChainStore<BlockHeader>>)) {
        // try first with in-memory store
        let in_memory_store: Arc<dyn ChainStore<BlockHeader>> = Arc::new(InMemConsensusStore::new());
        f(in_memory_store);

        // then with rocksdb store
        let tempdir = tempfile::tempdir().unwrap();
        let rw_store: Arc<dyn ChainStore<BlockHeader>> = Arc::new(initialise_test_rw_store(tempdir.path()));
        f(rw_store);
    }

    fn with_read_db(
        setup: impl Fn(Arc<dyn ChainStore<BlockHeader>>),
        assert: impl Fn(Arc<dyn ChainStore<BlockHeader>>, &dyn ReadOnlyChainStore<BlockHeader>),
    ) {
        let in_memory_store: Arc<dyn ChainStore<BlockHeader>> = Arc::new(InMemConsensusStore::new());
        // Initialize the store and take a snapshot
        setup(in_memory_store.clone());
        let snapshot = in_memory_store.snapshot();
        // check assertions against the in-memory snapshot
        assert(in_memory_store.clone(), snapshot.as_ref());

        let tempdir = tempfile::tempdir().unwrap();
        let rw_store: Arc<dyn ChainStore<BlockHeader>> = Arc::new(initialise_test_rw_store(tempdir.path()));
        // Initialize the store and take a snapshot
        setup(rw_store.clone());
        let snapshot = rw_store.snapshot();
        // check assertions against the RocksDB snapshot
        assert(rw_store.clone(), snapshot.as_ref());
    }

    fn with_db_path(f: impl Fn((Arc<dyn ChainStore<BlockHeader>>, &Path))) {
        let tempdir = tempfile::tempdir().unwrap();
        let rw_store: Arc<dyn ChainStore<BlockHeader>> = Arc::new(initialise_test_rw_store(tempdir.path()));
        f((rw_store, tempdir.path()));
    }

    fn sort_entries(mut v: Vec<(HeaderHash, Vec<HeaderHash>)>) -> Vec<(HeaderHash, Vec<HeaderHash>)> {
        v.sort_by_key(|(k, _)| *k);
        for (_, children) in &mut v {
            children.sort();
        }
        v
    }

    // from https://nick.groenen.me/notes/recursively-copy-files-in-rust/
    // NOTE: the stored database is only valid for Unix (Linux/MacOS) systems, so
    // any test relying on it should be guarded for not running on windows
    pub fn copy_recursively(source: impl AsRef<Path>, destination: impl AsRef<Path>) -> io::Result<()> {
        fs::create_dir_all(&destination)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            let filetype = entry.file_type()?;
            if filetype.is_dir() {
                copy_recursively(entry.path(), destination.as_ref().join(entry.file_name()))?;
            } else {
                fs::copy(entry.path(), destination.as_ref().join(entry.file_name()))?;
            }
        }
        Ok(())
    }
}
