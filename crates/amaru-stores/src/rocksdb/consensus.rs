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

use crate::chain_store::{ChainStore, ReadOnlyChainStore, StoreError};
use amaru_kernel::{Hash, RawBlock, cbor, from_cbor, to_cbor};
use amaru_ouroboros_traits::Nonces;
use amaru_ouroboros_traits::is_header::IsHeader;
use amaru_slot_arithmetic::EraHistory;
use rocksdb::{DB, IteratorMode, OptimisticTransactionDB, Options};
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
        let mut opts = Options::default();
        opts.create_if_missing(true);
        Ok(Self {
            db: OptimisticTransactionDB::open(&opts, basedir).map_err(|e| {
                StoreError::OpenError {
                    error: e.to_string(),
                }
            })?,
            basedir: basedir.clone(),
            era_history: era_history.clone(),
        })
    }

    pub fn open_for_readonly(basedir: &PathBuf) -> Result<ReadOnlyChainDB, StoreError> {
        let mut opts = Options::default();
        opts.create_if_missing(false);
        DB::open_for_read_only(&opts, basedir, false)
            .map_err(|e| StoreError::OpenError {
                error: e.to_string(),
            })
            .map(|db| ReadOnlyChainDB { db })
    }
}

const NONCES_PREFIX: [u8; 5] = [0x6e, 0x6f, 0x6e, 0x63, 0x65];

const BLOCK_PREFIX: [u8; 5] = [0x62, 0x6c, 0x6f, 0x63, 0x6b];

macro_rules! impl_ReadOnlyChainStore {
    (for $($s:ty),+) => {
        $(impl<H: IsHeader + for<'d> cbor::Decode<'d, ()>> ReadOnlyChainStore<H> for $s {
            fn load_header(&self, hash: &Hash<32>) -> Option<H> {
                self.db
                    .get_pinned(hash)
                    .ok()
                    .and_then(|bytes| from_cbor(bytes?.as_ref()))
            }

            fn get_children(&self, _hash: &Hash<32>) -> Vec<Hash<32>> {
                unimplemented!("get_children is not implemented for ReadOnlyStore")
            }

            fn get_root_hash(&self) -> Hash<32> {
                unimplemented!("get_root_hash is not implemented for ReadOnlyStore")
            }

            fn load_headers(&self) -> Vec<H> {
                let mut result = Vec::new();
                for values in self.db.iterator(IteratorMode::Start) {
                    let (_, value) = values.unwrap_or_default();
                    if let Some(header) = from_cbor(value.as_ref()) {
                        result.push(header);
                    }
                }
                result
            }

            fn count_headers(&self) -> usize {
                self.db.iterator(IteratorMode::Start).count()
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
    #[instrument(level = Level::TRACE, skip_all, fields(%hash))]
    fn store_header(&self, hash: &Hash<32>, header: &H) -> Result<(), StoreError> {
        self.db
            .put(hash, to_cbor(header))
            .map_err(|e| StoreError::WriteError {
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

    fn set_root_hash(&self, _hash: &Hash<32>) -> Result<(), StoreError> {
        unimplemented!("set_root_hash is not implemented for RocksDBStore")
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use amaru_kernel::tests::random_bytes;
    use amaru_kernel::{EraHistory, network::NetworkName};
    use amaru_ouroboros_traits::is_header::fake::FakeHeader;
    use std::{fs::create_dir_all, path::PathBuf};
    use tempfile::TempDir;

    fn init_dir_and_era(tempdir: &TempDir) -> (PathBuf, EraHistory) {
        let basedir = tempdir.path().join("rocksdb_chain_store");
        let era_history: &EraHistory = NetworkName::Testnet(42).into();
        (basedir, era_history.clone())
    }

    fn initialise_test_rw_store(tempdir: &TempDir) -> RocksDBStore {
        let (basedir, era_history) = init_dir_and_era(tempdir);
        create_dir_all(&basedir).unwrap();

        RocksDBStore::new(&basedir, &era_history).expect("fail to initialise RocksDB")
    }

    fn initialise_test_ro_store(tempdir: &TempDir) -> Result<ReadOnlyChainDB, StoreError> {
        let (basedir, _) = init_dir_and_era(tempdir);
        create_dir_all(&basedir).unwrap();

        RocksDBStore::open_for_readonly(&basedir)
    }

    #[test]
    fn rocksdb_chain_store_can_get_header_it_puts() {
        let tempdir = tempfile::tempdir().unwrap();
        let store = initialise_test_rw_store(&tempdir);

        let header = FakeHeader {
            block_number: 1,
            slot: 0,
            parent: None,
            body_hash: random_bytes(32).as_slice().into(),
        };

        store.store_header(&header.hash(), &header).unwrap();
        let header2 = store.load_header(&header.hash()).unwrap();
        assert_eq!(header, header2);
    }

    #[test]
    fn rocksdb_chain_store_can_get_block_it_puts() {
        let tempdir = tempfile::tempdir().unwrap();
        let store = initialise_test_rw_store(&tempdir);

        let hash: Hash<32> = random_bytes(32).as_slice().into();
        let block = RawBlock::from(&*vec![1; 64]);

        <RocksDBStore as ChainStore<FakeHeader>>::store_block(&store, &hash, &block).unwrap();
        let block2 =
            <RocksDBStore as ReadOnlyChainStore<FakeHeader>>::load_block(&store, &hash).unwrap();
        assert_eq!(block, block2);
    }

    #[test]
    fn rocksdb_chain_store_returns_not_found_for_nonexistent_block() {
        let tempdir = tempfile::tempdir().unwrap();
        let store = initialise_test_rw_store(&tempdir);

        let nonexistent_hash: Hash<32> = random_bytes(32).as_slice().into();

        let result =
            <RocksDBStore as ReadOnlyChainStore<FakeHeader>>::load_block(&store, &nonexistent_hash);

        assert_eq!(
            Err(StoreError::NotFound {
                hash: nonexistent_hash
            }),
            result
        );
    }

    #[test]
    fn both_rw_and_ro_can_be_open_on_same_dir() {
        let tempdir = tempfile::tempdir().unwrap();
        let _rw_store = initialise_test_rw_store(&tempdir);
        if let Err(e) = initialise_test_ro_store(&tempdir) {
            panic!("failed to re-open DB in read-only mode: {}", e);
        }
    }
}
