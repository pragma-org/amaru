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

use amaru_kernel::{HEADER_HASH_SIZE, Hash, ORIGIN_HASH, RawBlock, cbor, from_cbor, to_cbor};
use amaru_ouroboros_traits::is_header::IsHeader;
use amaru_ouroboros_traits::{ChainStore, Nonces, ReadOnlyChainStore, StoreError};
use amaru_slot_arithmetic::EraHistory;
use rocksdb::{ColumnFamilyDescriptor, DB, OptimisticTransactionDB, Options, SliceTransform};
use std::path::PathBuf;
use tracing::{Level, instrument};

pub struct RocksDBStore {
    pub basedir: PathBuf,
    era_history: EraHistory,
    db: OptimisticTransactionDB,
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
        // We must specify the existing CFs when opening read-only
        let cf_names = vec![DEFAULT_CF, CHILDREN_CF];
        let db = DB::open_cf_for_read_only(&opts, basedir, cf_names, false).map_err(|e| {
            StoreError::OpenError {
                error: e.to_string(),
            }
        })?;
        Ok(ReadOnlyChainDB { db })
    }
}

const DEFAULT_CF: &str = "default";

const CHILDREN_CF: &str = "children";

const CONSENSUS_PREFIX_LEN: usize = 5;

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
                self.db
                    .get_pinned(hash)
                    .ok()
                    .and_then(|bytes| from_cbor(bytes?.as_ref()))
            }

            fn get_children(&self, hash: &Hash<32>) -> Vec<Hash<32>> {
                let mut result = Vec::new();
                let prefix = [&CHILD_PREFIX[..], &hash[..]].concat();

                // Use the dedicated CF for children with a prefix-bound iterator
                if let Some(cf) = self.db.cf_handle(CHILDREN_CF) {
                    for res in self.db.prefix_iterator_cf(&cf, &prefix) {
                        match res {
                            Ok((key, _value)) => {
                                let mut arr = [0u8; 32];
                                arr.copy_from_slice(&key[(CONSENSUS_PREFIX_LEN + HEADER_HASH_SIZE)..]);
                                result.push(Hash::from(arr));
                            }
                            Err(err) => panic!("error iterating over children: {}", err),
                        }
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
                self.db.get_pinned(hash)
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
        tx.put(hash, to_cbor(header))
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
    use crate::in_memory::consensus::InMemConsensusStore;
    use amaru_kernel::tests::{random_bytes, random_hash};
    use amaru_ouroboros_traits::fake::tests;
    use amaru_ouroboros_traits::is_header::fake::FakeHeader;
    use proptest::strategy::{Strategy, ValueTree};
    use proptest::test_runner::TestRunner;
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
    fn store_parent_children_relationship_for_header() {
        with_db(|db| {
            let mut runner = TestRunner::default();

            // h0 -> h1 -> h2
            //      \
            //       -> h3
            let mut chain = tests::any_headers_chain_sized(3)
                .new_tree(&mut runner)
                .unwrap()
                .current();
            let mut h3 = tests::any_fake_header()
                .new_tree(&mut runner)
                .unwrap()
                .current();
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

    // HELPERS
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
}
