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

use amaru_kernel::ORIGIN_HASH;
use amaru_kernel::{HEADER_HASH_SIZE, Hash, RawBlock, cbor, from_cbor, to_cbor};
use amaru_ouroboros_traits::is_header::IsHeader;
use amaru_ouroboros_traits::{ChainStore, Nonces, ReadOnlyChainStore, StoreError};
use amaru_slot_arithmetic::EraHistory;
use rocksdb::{
    ColumnFamilyDescriptor, DB, Direction, IteratorMode, OptimisticTransactionDB, Options,
    SliceTransform,
};
use std::ops::Deref;
use std::path::PathBuf;
use tracing::{Level, instrument};

pub struct RocksDBStore {
    pub basedir: PathBuf,
    era_history: EraHistory,
    pub db: OptimisticTransactionDB,
}

pub struct ReadOnlyChainDB {
    db: DB,
}

impl RocksDBStore {
    pub fn new(basedir: &PathBuf, era_history: &EraHistory) -> Result<Self, StoreError> {
        // Default CF options (namespaced keys: blocks, nonces, anchor, best_chain)
        let mut default_opts = Options::default();
        default_opts.create_if_missing(true);
        default_opts
            .set_prefix_extractor(SliceTransform::create_fixed_prefix(CONSENSUS_PREFIX_LEN));

        // Children CF options (keys: "child" || parent_hash || child_hash)
        let mut children_opts = Options::default();
        children_opts.create_if_missing(true);
        children_opts.set_prefix_extractor(SliceTransform::create_fixed_prefix(
            CONSENSUS_PREFIX_LEN + HEADER_HASH_SIZE,
        ));

        let cfs = vec![
            ColumnFamilyDescriptor::new(DEFAULT_CF, default_opts),
            ColumnFamilyDescriptor::new(CHILDREN_CF, children_opts),
        ];

        let mut opts = Options::default();
        opts.create_if_missing(true);
        opts.create_missing_column_families(true);
        let db =
            OptimisticTransactionDB::open_cf_descriptors(&opts, basedir, cfs).map_err(|e| {
                StoreError::OpenError {
                    error: e.to_string(),
                }
            })?;

        Ok(Self {
            db,
            basedir: basedir.clone(),
            era_history: era_history.clone(),
        })
    }

    pub fn open_for_readonly(basedir: &PathBuf) -> Result<ReadOnlyChainDB, StoreError> {
        let mut opts = Options::default();
        opts.create_if_missing(false);
        let cf_names = vec![DEFAULT_CF, CHILDREN_CF];
        let db = DB::open_cf_for_read_only(&opts, basedir, cf_names, false).map_err(|e| {
            StoreError::OpenError {
                error: e.to_string(),
            }
        })?;
        Ok(ReadOnlyChainDB { db })
    }

    pub fn create_transaction(&self) -> rocksdb::Transaction<'_, OptimisticTransactionDB> {
        self.db.transaction()
    }
}

const DEFAULT_CF: &str = "default";

const CHILDREN_CF: &str = "children";

const CONSENSUS_PREFIX_LEN: usize = 5;

/// "heade"
const HEADER_PREFIX: [u8; CONSENSUS_PREFIX_LEN] = [0x68, 0x65, 0x61, 0x64, 0x65];

/// "nonce"
const NONCES_PREFIX: [u8; CONSENSUS_PREFIX_LEN] = [0x6e, 0x6f, 0x6e, 0x63, 0x65];

/// "block"
const BLOCK_PREFIX: [u8; CONSENSUS_PREFIX_LEN] = [0x62, 0x6c, 0x6f, 0x63, 0x6b];

/// "ancho"
const ANCHOR_PREFIX: [u8; CONSENSUS_PREFIX_LEN] = [0x61, 0x6e, 0x63, 0x68, 0x6f];

/// "best_"
const BEST_CHAIN_PREFIX: [u8; CONSENSUS_PREFIX_LEN] = [0x62, 0x65, 0x73, 0x74, 0x5f];

/// "child"
const CHILD_PREFIX: [u8; CONSENSUS_PREFIX_LEN] = [0x63, 0x68, 0x69, 0x6c, 0x64];

macro_rules! impl_ReadOnlyChainStore {
    (for $($s:ty),+) => {
        $(impl<H: IsHeader + for<'d> cbor::Decode<'d, ()>> ReadOnlyChainStore<H> for $s {
            fn load_header(&self, hash: &Hash<32>) -> Option<H> {
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

            fn load_nonces(&self) -> Box<dyn Iterator<Item=(Hash<HEADER_HASH_SIZE>, Nonces)> + '_> {
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

            fn load_blocks(&self) -> Box<dyn Iterator<Item=(Hash<HEADER_HASH_SIZE>, RawBlock)> + '_> {
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

            fn load_parents_children(&self) -> Box<dyn Iterator<Item=(Hash<HEADER_HASH_SIZE>, Vec<Hash<HEADER_HASH_SIZE>>)> + '_> {
                let cf = self.db.cf_handle(CHILDREN_CF).expect("missing children CF");
                let iter = self
                    .db
                    .iterator_cf(&cf, IteratorMode::From(&CHILD_PREFIX, Direction::Forward));

                let mut groups: Vec<(Hash<HEADER_HASH_SIZE>, Vec<Hash<HEADER_HASH_SIZE>>)> = Vec::new();
                let mut current_parent: Option<Hash<HEADER_HASH_SIZE>> = None;
                let mut current_children: Vec<Hash<HEADER_HASH_SIZE>> = Vec::new();

                for kv in iter {
                    let (k, _v) = kv.expect("error iterating over children CF");

                    // Stop once we exit the "child" namespace
                    if !k.starts_with(&CHILD_PREFIX) {
                        break;
                    }

                    // Key layout: [CHILD_PREFIX][parent][child]
                    let parent_start = CONSENSUS_PREFIX_LEN;
                    let parent_end = parent_start + HEADER_HASH_SIZE;
                    let child_start = parent_end;
                    let child_end = child_start + HEADER_HASH_SIZE;
                    if k.len() < child_end { continue; }

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

            fn get_children(&self, hash: &Hash<32>) -> Vec<Hash<32>> {
                let mut result = Vec::new();
                let prefix = [&CHILD_PREFIX[..], &hash[..]].concat();
                let cf = self.db.cf_handle(CHILDREN_CF).expect("missing children CF");
                for res in self.db.prefix_iterator_cf(&cf, &prefix) {
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

            fn get_anchor_hash(&self) -> Hash<32> {
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

            fn get_best_chain_hash(&self) -> Hash<32> {
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

            fn has_header(&self, hash: &Hash<32>) -> bool {
                let prefix = [&HEADER_PREFIX[..], &hash[..]].concat();
                self.db.get_pinned(prefix)
                    .map(|opt| opt.is_some())
                    .unwrap_or(false)
            }

            fn get_nonces(&self, header: &Hash<32>) -> Option<Nonces> {
                self.db
                    .get_pinned([&NONCES_PREFIX[..], &header[..]].concat())
                    .ok()
                    .flatten()
                    .as_deref()
                    .and_then(from_cbor)
            }

            fn load_block(&self, hash: &Hash<32>) -> Result<RawBlock, StoreError> {
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

impl<H: IsHeader + for<'d> cbor::Decode<'d, ()>> ChainStore<H> for RocksDBStore {
    #[instrument(level = Level::TRACE, skip_all, fields(header = header.hash().to_string()))]
    fn store_header(&self, header: &H) -> Result<(), StoreError> {
        let hash = header.hash();
        let tx = self.db.transaction();
        if let Some(parent) = header.parent()
            && let Some(cf) = self.db.cf_handle(CHILDREN_CF)
        {
            tx.put_cf(
                &cf,
                [&CHILD_PREFIX[..], &parent[..], &hash[..]].concat(),
                [],
            )
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

    fn put_nonces(&self, header: &Hash<32>, nonces: &Nonces) -> Result<(), StoreError> {
        self.db
            .put([&NONCES_PREFIX[..], &header[..]].concat(), to_cbor(nonces))
            .map_err(|e| StoreError::WriteError {
                error: e.to_string(),
            })
    }

    fn era_history(&self) -> &EraHistory {
        &self.era_history
    }

    fn store_block(&self, hash: &Hash<32>, block: &RawBlock) -> Result<(), StoreError> {
        self.db
            .put([&BLOCK_PREFIX[..], &hash[..]].concat(), block.as_ref())
            .map_err(|e| StoreError::WriteError {
                error: e.to_string(),
            })
    }

    fn remove_header(&self, hash: &Hash<32>) -> Result<(), StoreError> {
        self.db.delete(hash).map_err(|e| StoreError::WriteError {
            error: e.to_string(),
        })
    }

    fn set_anchor_hash(&self, hash: &Hash<32>) -> Result<(), StoreError> {
        self.db
            .put(ANCHOR_PREFIX, hash.as_ref())
            .map_err(|e| StoreError::WriteError {
                error: e.to_string(),
            })
    }

    fn set_best_chain_hash(&self, hash: &Hash<32>) -> Result<(), StoreError> {
        self.db
            .put(BEST_CHAIN_PREFIX, hash.as_ref())
            .map_err(|e| StoreError::WriteError {
                error: e.to_string(),
            })
    }
    fn update_best_chain(&self, anchor: &Hash<32>, tip: &Hash<32>) -> Result<(), StoreError> {
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

#[cfg(any(test, feature = "test-utils"))]
#[expect(clippy::unwrap_used)]
#[expect(clippy::expect_used)]
pub fn initialise_test_rw_store(path: &std::path::Path) -> RocksDBStore {
    use std::fs::create_dir_all;
    let (basedir, era_history) = init_dir_and_era(path);
    create_dir_all(&basedir).unwrap();
    RocksDBStore::new(&basedir, &era_history).expect("fail to initialise RocksDB")
}

#[cfg(any(test, feature = "test-utils"))]
#[expect(clippy::unwrap_used)]
pub fn initialise_test_ro_store(path: &std::path::Path) -> Result<ReadOnlyChainDB, StoreError> {
    use std::fs::create_dir_all;
    let (basedir, _) = init_dir_and_era(path);
    create_dir_all(&basedir).unwrap();
    RocksDBStore::open_for_readonly(&basedir)
}

#[cfg(any(test, feature = "test-utils"))]
fn init_dir_and_era(path: &std::path::Path) -> (PathBuf, EraHistory) {
    use amaru_kernel::network::NetworkName;
    let basedir = path.join("rocksdb_chain_store");
    let era_history: &EraHistory = NetworkName::Testnet(42).into();
    (basedir, era_history.clone())
}

#[cfg(test)]
pub mod test {
    use super::*;
    use amaru_kernel::tests::{random_bytes, random_hash};
    use amaru_kernel::{Nonce, ORIGIN_HASH};
    use amaru_ouroboros_traits::fake::tests::{any_fake_header, any_headers_chain_sized};
    use amaru_ouroboros_traits::in_memory_consensus_store::InMemConsensusStore;
    use amaru_ouroboros_traits::is_header::fake::FakeHeader;
    use proptest::strategy::{Strategy, ValueTree};
    use proptest::test_runner::TestRunner;
    use std::collections::BTreeMap;
    use std::sync::Arc;

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
            let header = FakeHeader {
                block_number: 1,
                slot: 0,
                parent: None,
                body_hash: random_bytes(32).as_slice().into(),
            };

            db.store_header(&header).unwrap();
            let header2 = db.load_header(&header.hash()).unwrap();
            assert_eq!(header, header2);
        })
    }

    #[test]
    fn rocksdb_chain_store_can_get_block_it_puts() {
        with_db(|db| {
            let hash: Hash<32> = random_bytes(32).as_slice().into();
            let block = RawBlock::from(&*vec![1; 64]);

            db.store_block(&hash, &block).unwrap();
            let block2 = db.load_block(&hash).unwrap();
            assert_eq!(block, block2);
        })
    }

    #[test]
    fn rocksdb_chain_store_returns_not_found_for_nonexistent_block() {
        with_db(|db| {
            let nonexistent_hash: Hash<32> = random_bytes(32).as_slice().into();
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
            let mut chain = run(any_headers_chain_sized(3));
            let mut h3 = run(any_fake_header());
            h3.parent = Some(chain[1].hash());
            chain.push(h3);

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
            let mut headers: Vec<FakeHeader> = vec![];
            for i in 0..10usize {
                let header = FakeHeader {
                    block_number: i as u64,
                    slot: i as u64 * 10,
                    parent: if i == 0 {
                        None
                    } else {
                        Some(headers[i - 1].hash())
                    },
                    body_hash: random_bytes(32).as_slice().into(),
                };
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
            let mut chain = run(any_headers_chain_sized(3));
            let mut h3 = run(any_fake_header());
            h3.parent = Some(chain[1].hash());
            chain.push(h3);
            let mut h4 = run(any_fake_header());
            h4.parent = Some(h3.hash());
            chain.push(h4);

            let mut expected = BTreeMap::new();

            for header in &chain {
                if let Some(parent) = header.parent {
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
            let chain = run(any_headers_chain_sized(3));
            let mut expected = BTreeMap::new();
            for header in &chain {
                let nonces = Nonces {
                    active: Nonce::from(random_bytes(32).as_slice()),
                    evolving: Nonce::from(random_bytes(32).as_slice()),
                    candidate: Nonce::from(random_bytes(32).as_slice()),
                    tail: header.parent.unwrap_or(ORIGIN_HASH),
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
            let chain = run(any_headers_chain_sized(3));
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
            let chain = run(any_headers_chain_sized(15));
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

    // HELPERS
    fn run<T>(s: impl Strategy<Value = T>) -> T {
        let mut runner = TestRunner::default();
        s.new_tree(&mut runner).unwrap().current()
    }

    fn with_db(f: impl Fn(Arc<dyn ChainStore<FakeHeader>>)) {
        // try first with in-memory store
        let in_memory_store: Arc<dyn ChainStore<FakeHeader>> = Arc::new(InMemConsensusStore::new());
        f(in_memory_store);

        // then with rocksdb store
        let tempdir = tempfile::tempdir().unwrap();
        let rw_store: Arc<dyn ChainStore<FakeHeader>> =
            Arc::new(initialise_test_rw_store(tempdir.path()));
        f(rw_store);
    }

    fn sort_entries(
        mut v: Vec<(Hash<HEADER_HASH_SIZE>, Vec<Hash<HEADER_HASH_SIZE>>)>,
    ) -> Vec<(Hash<HEADER_HASH_SIZE>, Vec<Hash<HEADER_HASH_SIZE>>)> {
        v.sort_by_key(|(k, _)| *k);
        for (_, children) in &mut v {
            children.sort();
        }
        v
    }
}
