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

pub mod migration;
pub mod util;

pub use migration::*;

use amaru_kernel::{
    HEADER_HASH_SIZE, Hash, HeaderHash, ORIGIN_HASH, RawBlock, cbor, from_cbor, to_cbor,
};
use amaru_ouroboros_traits::is_header::IsHeader;
use amaru_ouroboros_traits::{ChainStore, Nonces, ReadOnlyChainStore, StoreError};
use rocksdb::{DB, IteratorMode, OptimisticTransactionDB, Options, PrefixRange, ReadOptions};
use std::fs::{self};
use std::ops::Deref;
use std::path::PathBuf;
use tracing::{Level, instrument};

use crate::rocksdb::RocksDbConfig;
use crate::rocksdb::consensus::util::{
    ANCHOR_PREFIX, BEST_CHAIN_PREFIX, BLOCK_PREFIX, CHAIN_DB_VERSION, CHILD_PREFIX,
    CONSENSUS_PREFIX_LEN, HEADER_PREFIX, NONCES_PREFIX, open_db,
};

pub struct RocksDBStore {
    pub basedir: PathBuf,
    pub db: OptimisticTransactionDB,
}

pub struct ReadOnlyChainDB {
    db: DB,
}

impl RocksDBStore {
    pub fn new(config: RocksDbConfig) -> Result<Self, StoreError> {
        let (basedir, db) = open_db(&config)?;

        check_db_version(&db)?;

        Ok(Self { db, basedir })
    }

    /// Create a Chain DB at the given path.
    /// If the database already exists, an error will be raised.
    /// To check the existence of the database we only check the directory pointed at
    /// contains at least one file.
    /// NOTE: There should be a better way to detect whether or not a directory contains
    /// a RocksDB database.
    pub fn create(config: RocksDbConfig) -> Result<Self, StoreError> {
        let basedir = config.dir.clone();
        let list = fs::read_dir(&basedir);
        if let Ok(entries) = list {
            if entries.count() > 0 {
                return Err(StoreError::OpenError {
                    error: format!(
                        "Cannot create RocksDB at {}, directory is not empty",
                        basedir.display()
                    ),
                });
            }
        }

        let (_, db) = open_db(&config)?;
        set_version(&db)?;

        Ok(Self { db, basedir })
    }

    pub fn open_for_readonly(config: RocksDbConfig) -> Result<ReadOnlyChainDB, StoreError> {
        let basedir = config.dir.clone();
        let opts: Options = config.into();
        DB::open_for_read_only(&opts, basedir, false)
            .map_err(|e| StoreError::OpenError {
                error: e.to_string(),
            })
            .map(|db| ReadOnlyChainDB { db })
    }

    pub fn create_transaction(&self) -> rocksdb::Transaction<'_, OptimisticTransactionDB> {
        self.db.transaction()
    }
}

pub fn check_db_version(db: &OptimisticTransactionDB) -> Result<(), StoreError> {
    let version = db.get(VERSION_KEY).map_err(|e| StoreError::OpenError {
        error: e.to_string(),
    })?;

    match version {
        Some(v) => {
            let stored = ((v[0] as u16) << 8) | v[1] as u16;
            if stored != CHAIN_DB_VERSION {
                Err(StoreError::IncompatibleDbVersions {
                    stored,
                    current: CHAIN_DB_VERSION,
                })
            } else {
                Ok(())
            }
        }
        None => Err(StoreError::IncompatibleDbVersions {
            stored: 0,
            current: CHAIN_DB_VERSION,
        }),
    }
}

macro_rules! impl_ReadOnlyChainStore {
    (for $($s:ty),+) => {
        $(impl<H: IsHeader + Clone + for<'d> cbor::Decode<'d, ()>> ReadOnlyChainStore<H> for $s {
            fn load_header(&self, hash: &HeaderHash) -> Option<H> {
                let prefix = [&HEADER_PREFIX[..], &hash[..]].concat();
                self.db
                    .get_pinned(prefix)
                    .ok()
                    .and_then(|bytes| from_cbor(bytes?.as_ref()))
            }

            fn load_headers(&self) -> Box<dyn Iterator<Item=H> + '_> {
                Box::new(self.db
                    .prefix_iterator(&HEADER_PREFIX).filter_map(|item| {
                    match item {
                        Ok((_k, v)) => {
                            from_cbor(v.as_ref())
                        }
                        Err(err) => panic!("error iterating over headers: {}", err),
                    }
                }).into_iter())
            }

            fn load_nonces(&self) -> Box<dyn Iterator<Item=(HeaderHash, Nonces)> + '_> {
                Box::new(self.db
                    .prefix_iterator(&NONCES_PREFIX).filter_map(|item| {
                    match item {
                        Ok((k, v)) => {
                            let hash = Hash::from(&k[CONSENSUS_PREFIX_LEN..]);
                            if let Some(nonces) = from_cbor(&v) {
                                Some((hash, nonces))
                            } else {
                                None
                            }
                        }
                        Err(err) => panic!("error iterating over nonces: {}", err),
                    }
                }).into_iter())
            }

            fn load_blocks(&self) -> Box<dyn Iterator<Item=(HeaderHash, RawBlock)> + '_> {
                Box::new(self.db
                    .prefix_iterator(&BLOCK_PREFIX).map(|item| {
                    match item {
                        Ok((k, v)) => {
                            let hash = Hash::from(&k[CONSENSUS_PREFIX_LEN..]);
                                (hash, RawBlock::from(v.deref()))
                        }
                        Err(err) => panic!("error iterating over blocks: {}", err),
                    }
                }).into_iter())
            }

            fn load_parents_children(&self) -> Box<dyn Iterator<Item=(HeaderHash, Vec<HeaderHash>)> + '_> {
                let mut groups: Vec<(HeaderHash, Vec<HeaderHash>)> = Vec::new();
                let mut current_parent: Option<HeaderHash> = None;
                let mut current_children: Vec<HeaderHash> = Vec::new();

                for kv in self
                    .db
                    .prefix_iterator(&CHILD_PREFIX) {
                    let (k, _v) = kv.expect("error iterating over children keys");

                    // Key layout: [CHILD_PREFIX][parent][child]
                    let parent_start = CONSENSUS_PREFIX_LEN;
                    let parent_end = parent_start + HEADER_HASH_SIZE;
                    let child_start = parent_end;
                    let child_end = child_start + HEADER_HASH_SIZE;

                    let mut parent_arr = [0u8; HEADER_HASH_SIZE];
                    parent_arr.copy_from_slice(&k[parent_start..parent_end]);
                    let parent_hash = Hash::from(parent_arr);

                    let mut child_arr = [0u8; HEADER_HASH_SIZE];
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

            fn get_children(&self, hash: &HeaderHash) -> Vec<HeaderHash> {
                let mut result = Vec::new();
                let mut opts = ReadOptions::default();
                opts.set_iterate_range(PrefixRange([&CHILD_PREFIX[..], &hash[..]].concat()));

                for res in self.db.iterator_opt(IteratorMode::Start, opts) {
                    match res {
                        Ok((key, _value)) => {
                            let mut arr = [0u8; HEADER_HASH_SIZE];
                            arr.copy_from_slice(&key[(CONSENSUS_PREFIX_LEN + HEADER_HASH_SIZE)..]);
                            result.push(Hash::from(arr));
                        }
                        Err(err) => panic!("error iterating over children: {}", err),
                    }
                }
                result
            }

            fn get_anchor_hash(&self) -> HeaderHash {
                self.db
                    .get_pinned(&ANCHOR_PREFIX)
                    .ok()
                    .flatten()
                    .and_then(|bytes| {
                        if bytes.len() == HEADER_HASH_SIZE {
                            Some(Hash::from(bytes.as_ref()))
                        } else {
                            None
                        }
                    })
                    .unwrap_or(ORIGIN_HASH)
            }

            fn get_best_chain_hash(&self) -> HeaderHash {
                self.db
                    .get_pinned(&BEST_CHAIN_PREFIX)
                    .ok()
                    .flatten()
                    .and_then(|bytes| {
                        if bytes.len() == HEADER_HASH_SIZE {
                            Some(Hash::from(bytes.as_ref()))
                        } else {
                            None
                        }
                    })
                    .unwrap_or(ORIGIN_HASH)
            }

            fn has_header(&self, hash: &HeaderHash) -> bool {
                let prefix = [&HEADER_PREFIX[..], &hash[..]].concat();
                self.db.get_pinned(prefix)
                    .map(|opt| opt.is_some())
                    .unwrap_or(false)
            }

            fn get_nonces(&self, header: &HeaderHash) -> Option<Nonces> {
                self.db
                    .get_pinned([&NONCES_PREFIX[..], &header[..]].concat())
                    .ok()
                    .flatten()
                    .as_deref()
                    .and_then(from_cbor)
            }

            fn load_block(&self, hash: &HeaderHash) -> Result<RawBlock, StoreError> {
                self.db
                    .get_pinned([&BLOCK_PREFIX[..], &hash[..]].concat())
                    .map_err(|e| StoreError::ReadError {
                        error: e.to_string(),
                    })?
                    .ok_or(StoreError::NotFound { hash: *hash })
                    .map(|bytes| bytes.as_ref().into())
            }

        })*
    }
}

impl_ReadOnlyChainStore!(for ReadOnlyChainDB, RocksDBStore);

impl<H: IsHeader + Clone + for<'d> cbor::Decode<'d, ()>> ChainStore<H> for RocksDBStore {
    #[instrument(level = Level::TRACE, skip_all, fields(header = header.hash().to_string()))]
    fn store_header(&self, header: &H) -> Result<(), StoreError> {
        let hash = header.hash();
        let tx = self.db.transaction();
        if let Some(parent) = header.parent() {
            tx.put([&CHILD_PREFIX[..], &parent[..], &hash[..]].concat(), [])
                .map_err(|e| StoreError::WriteError {
                    error: e.to_string(),
                })?;
        };
        tx.put([&HEADER_PREFIX[..], &hash[..]].concat(), to_cbor(header))
            .map_err(|e| StoreError::WriteError {
                error: e.to_string(),
            })?;
        tx.commit().map_err(|e| StoreError::WriteError {
            error: e.to_string(),
        })
    }

    fn put_nonces(&self, header: &HeaderHash, nonces: &Nonces) -> Result<(), StoreError> {
        self.db
            .put([&NONCES_PREFIX[..], &header[..]].concat(), to_cbor(nonces))
            .map_err(|e| StoreError::WriteError {
                error: e.to_string(),
            })
    }

    fn store_block(&self, hash: &HeaderHash, block: &RawBlock) -> Result<(), StoreError> {
        self.db
            .put([&BLOCK_PREFIX[..], &hash[..]].concat(), block.as_ref())
            .map_err(|e| StoreError::WriteError {
                error: e.to_string(),
            })
    }

    fn set_anchor_hash(&self, hash: &HeaderHash) -> Result<(), StoreError> {
        self.db
            .put(ANCHOR_PREFIX, hash.as_ref())
            .map_err(|e| StoreError::WriteError {
                error: e.to_string(),
            })
    }

    fn set_best_chain_hash(&self, hash: &HeaderHash) -> Result<(), StoreError> {
        self.db
            .put(BEST_CHAIN_PREFIX, hash.as_ref())
            .map_err(|e| StoreError::WriteError {
                error: e.to_string(),
            })
    }
    fn update_best_chain(&self, anchor: &HeaderHash, tip: &HeaderHash) -> Result<(), StoreError> {
        let tx = self.db.transaction();
        tx.put(ANCHOR_PREFIX, anchor.as_ref())
            .map_err(|e| StoreError::WriteError {
                error: e.to_string(),
            })?;
        tx.put(BEST_CHAIN_PREFIX, tip.as_ref())
            .map_err(|e| StoreError::WriteError {
                error: e.to_string(),
            })?;
        tx.commit().map_err(|e| StoreError::WriteError {
            error: e.to_string(),
        })
    }
}

#[cfg(test)]
pub mod test {
    use crate::rocksdb::consensus::migration::migrate_db_path;

    use super::*;
    use amaru_kernel::tests::{random_bytes, random_hash};
    use amaru_kernel::{HeaderHash, Nonce, ORIGIN_HASH};
    use amaru_ouroboros_traits::ChainStore;
    use amaru_ouroboros_traits::is_header::BlockHeader;
    use amaru_ouroboros_traits::tests::{
        any_header_with_parent, any_headers_chain, make_header, run,
    };
    use std::collections::BTreeMap;
    use std::path::Path;
    use std::sync::Arc;
    use std::{fs, io};

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
            assert_eq!(block, block2);
        })
    }

    #[test]
    fn rocksdb_chain_store_returns_not_found_for_nonexistent_block() {
        with_db(|db| {
            let nonexistent_hash: HeaderHash = random_bytes(32).as_slice().into();
            let result = db.load_block(&nonexistent_hash);

            assert_eq!(
                Err(StoreError::NotFound {
                    hash: nonexistent_hash
                }),
                result
            );
        });
    }

    #[test]
    fn best_chain_hash_when_store_is_empty() {
        with_db(|db| assert_eq!(db.get_best_chain_hash(), ORIGIN_HASH))
    }

    #[test]
    fn store_best_chain_hash() {
        with_db(|db| {
            let best_chain = random_hash();
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
            let anchor = random_hash();
            db.set_anchor_hash(&anchor).unwrap();
            assert_eq!(db.get_anchor_hash(), anchor);
        })
    }

    #[test]
    fn update_best_chain() {
        with_db(|db| {
            let anchor = random_hash();
            let tip = random_hash();
            db.update_best_chain(&anchor, &tip).unwrap();
            assert_eq!(db.get_anchor_hash(), anchor);
            assert_eq!(db.get_best_chain_hash(), tip);
        })
    }

    #[test]
    fn store_parent_children_relationship_for_header() {
        with_db(|db| {
            // h0 -> h1 -> h2
            //      \
            //       -> h3
            let mut chain = run(any_headers_chain(3));
            let h3 = run(any_header_with_parent(chain[1].hash()));
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
        with_db(|db| {
            let mut headers: Vec<BlockHeader> = vec![];
            for i in 0..10usize {
                let parent = if i == 0 {
                    None
                } else {
                    Some(headers[i - 1].hash())
                };
                let header = make_header(i as u64, i as u64 * 10, parent).into();
                db.store_header(&header).unwrap();
                headers.push(header);
            }
            headers.sort();
            let mut result = db.load_headers().collect::<Vec<_>>();
            result.sort();
            assert_eq!(result, headers);
        })
    }

    #[test]
    fn load_parents_children() {
        with_db(|db| {
            // h0 -> h1 -> h2
            //      \
            //       -> h3 -> h4
            let mut chain = run(any_headers_chain(3));
            let h3 = run(any_header_with_parent(chain[1].hash()));
            chain.push(h3.clone());
            let h4 = run(any_header_with_parent(h3.hash()));
            chain.push(h4);

            let mut expected = BTreeMap::new();

            for header in &chain {
                if let Some(parent) = header.parent() {
                    expected
                        .entry(parent)
                        .or_insert_with(Vec::new)
                        .push(header.hash());
                }
                db.store_header(header).unwrap();
            }

            let result = sort_entries(db.load_parents_children().collect::<Vec<_>>());
            let expected = sort_entries(expected.into_iter().collect::<Vec<_>>());
            assert_eq!(result, expected);
        })
    }

    #[test]
    fn load_nonces() {
        with_db(|db| {
            let chain = run(any_headers_chain(3));
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

            let mut result = db.load_nonces().collect::<Vec<_>>();
            result.sort();
            let mut expected = expected.into_iter().collect::<Vec<_>>();
            expected.sort();
            assert_eq!(result, expected);
        })
    }

    #[test]
    fn load_blocks() {
        with_db(|db| {
            let chain = run(any_headers_chain(3));
            let mut expected = BTreeMap::new();
            for header in &chain {
                let block = RawBlock::from(random_bytes(32).as_slice());
                db.store_block(&header.hash(), &block).unwrap();
                expected.insert(header.hash(), block);
            }

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
            let chain = run(any_headers_chain(15));
            for header in &chain {
                db.store_header(header).unwrap();
            }

            // set the chain anchor to the 5th header
            // and the tip to the last header
            db.set_anchor_hash(&chain[4].hash()).unwrap();
            db.set_best_chain_hash(&chain[14].hash()).unwrap();
            let result = db.retrieve_best_chain();
            assert_eq!(
                result,
                chain[4..].iter().map(|h| h.hash()).collect::<Vec<_>>()
            );
        })
    }

    // MIGRATIONS

    #[test]
    fn fails_to_open_rw_db_if_stored_version_does_not_exist() {
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path();
        let basedir = init_dir(path);
        let config = RocksDbConfig::new(basedir);

        let result = RocksDBStore::new(config);
        match result {
            Err(StoreError::IncompatibleDbVersions { stored, current }) => {
                assert_eq!(stored, 0);
                assert_eq!(current, CHAIN_DB_VERSION);
            }
            Err(e) => panic!("Expected IncompatibleDbVersions error, got: {:?}", e),
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

        let store = RocksDBStore::create(RocksDbConfig::new(basedir))
            .expect("should create DB successfully");
        let version = get_version(&store.db).expect("should read version successfully");

        assert_eq!(version, CHAIN_DB_VERSION);
    }

    #[test]
    fn can_convert_v0_sample_db_to_v1() {
        let tempdir = tempfile::tempdir().unwrap();
        let target = tempdir.path();
        let config = RocksDbConfig::new(target.to_path_buf());
        let source = PathBuf::from("sample-chain-db/v0");

        copy_recursively(source, target).unwrap();

        let result = migrate_db_path(target).expect("Migration should succeed");

        let db = RocksDBStore::new(config)
            .expect("DB should successfully be opened as it's been migrated");
        assert_eq!((0, 1), result);
        let header: Option<BlockHeader> = db.load_header(&HeaderHash::from(
            hex::decode(SAMPLE_HASH).unwrap().as_slice(),
        ));
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

    const SAMPLE_HASH: &str = "2e78d1386ae414e62c72933c753a1cc5f6fdaefe0e6f0ee462bee8bb24285c1b";
    // HELPERS

    pub fn initialise_test_rw_store(path: &std::path::Path) -> RocksDBStore {
        let basedir = init_dir(path);
        let config = RocksDbConfig::new(basedir);

        RocksDBStore::create(config).expect("fail to initialise RocksDB")
    }

    pub fn initialise_test_ro_store(path: &std::path::Path) -> Result<ReadOnlyChainDB, StoreError> {
        let basedir = init_dir(path);
        RocksDBStore::open_for_readonly(RocksDbConfig::new(basedir))
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
        let chain = run(any_headers_chain(10));
        for header in &chain {
            let block = RawBlock::from(random_bytes(32).as_slice());
            db.store_header(header).unwrap();
            <RocksDBStore as ChainStore<BlockHeader>>::store_block(&db, &header.hash(), &block)
                .unwrap();
            let nonces = Nonces {
                active: Nonce::from(random_bytes(32).as_slice()),
                evolving: Nonce::from(random_bytes(32).as_slice()),
                candidate: Nonce::from(random_bytes(32).as_slice()),
                tail: header.parent().unwrap_or(ORIGIN_HASH),
                epoch: Default::default(),
            };
            <RocksDBStore as ChainStore<BlockHeader>>::put_nonces(&db, &header.hash(), &nonces)
                .unwrap();
        }
    }

    fn with_db(f: impl Fn(Arc<dyn ChainStore<BlockHeader>>)) {
        // // try first with in-memory store
        // let in_memory_store: Arc<dyn ChainStore<BlockHeader>> =
        //     Arc::new(InMemConsensusStore::new());
        // f(in_memory_store);

        // then with rocksdb store
        let tempdir = tempfile::tempdir().unwrap();
        let rw_store: Arc<dyn ChainStore<BlockHeader>> =
            Arc::new(initialise_test_rw_store(tempdir.path()));
        f(rw_store);
    }

    fn sort_entries(
        mut v: Vec<(HeaderHash, Vec<HeaderHash>)>,
    ) -> Vec<(HeaderHash, Vec<HeaderHash>)> {
        v.sort_by_key(|(k, _)| *k);
        for (_, children) in &mut v {
            children.sort();
        }
        v
    }

    // from https://nick.groenen.me/notes/recursively-copy-files-in-rust/
    pub fn copy_recursively(
        source: impl AsRef<Path>,
        destination: impl AsRef<Path>,
    ) -> io::Result<()> {
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
