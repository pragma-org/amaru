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

use amaru_consensus::{
    consensus::store::{ChainStore, StoreError},
    Nonces,
};
use amaru_kernel::{cbor, from_cbor, to_cbor, Hash, RawBlock};
use amaru_ouroboros_traits::is_header::IsHeader;
use rocksdb::{OptimisticTransactionDB, Options};
use slot_arithmetic::EraHistory;
use std::{collections::BTreeMap, path::PathBuf};
use tracing::{instrument, Level};

pub struct RocksDBStore {
    pub basedir: PathBuf,
    era_history: EraHistory,
    db: OptimisticTransactionDB,
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
}

const NONCES_PREFIX: [u8; 5] = [0x6e, 0x6f, 0x6e, 0x63, 0x65];

const BLOCK_PREFIX: [u8; 5] = [0x62, 0x6c, 0x6f, 0x63, 0x6b];

impl<H: IsHeader + for<'d> cbor::Decode<'d, ()>> ChainStore<H> for RocksDBStore {
    fn load_header(&self, hash: &Hash<32>) -> Option<H> {
        self.db
            .get_pinned(hash)
            .ok()
            .and_then(|bytes| from_cbor(bytes?.as_ref()))
    }

    #[instrument(level = Level::TRACE, skip_all, fields(%hash))]
    fn store_header(&mut self, hash: &Hash<32>, header: &H) -> Result<(), StoreError> {
        self.db
            .put(hash, to_cbor(header))
            .map_err(|e| StoreError::WriteError {
                error: e.to_string(),
            })
    }

    fn get_nonces(&self, header: &Hash<32>) -> Option<Nonces> {
        self.db
            .get_pinned([&NONCES_PREFIX[..], &header[..]].concat())
            .ok()
            .flatten()
            .as_deref()
            .and_then(from_cbor)
    }

    fn put_nonces(&mut self, header: &Hash<32>, nonces: &Nonces) -> Result<(), StoreError> {
        self.db
            .put([&NONCES_PREFIX[..], &header[..]].concat(), to_cbor(nonces))
            .map_err(|e| StoreError::WriteError {
                error: e.to_string(),
            })
    }

    fn era_history(&self) -> &EraHistory {
        &self.era_history
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

    fn store_block(&mut self, hash: &Hash<32>, block: &RawBlock) -> Result<(), StoreError> {
        self.db
            .put([&BLOCK_PREFIX[..], &hash[..]].concat(), block)
            .map_err(|e| StoreError::WriteError {
                error: e.to_string(),
            })
    }
}

pub struct InMemConsensusStore<H> {
    nonces: BTreeMap<Hash<32>, Nonces>,
    headers: BTreeMap<Hash<32>, H>,
    blocks: BTreeMap<Hash<32>, RawBlock>,
    era_history: EraHistory,
}

impl<H> Default for InMemConsensusStore<H> {
    fn default() -> Self {
        Self::new()
    }
}

impl<H> InMemConsensusStore<H> {
    pub fn new() -> InMemConsensusStore<H> {
        InMemConsensusStore {
            nonces: BTreeMap::new(),
            headers: BTreeMap::new(),
            blocks: BTreeMap::new(),
            era_history: EraHistory::default(),
        }
    }
}

impl<H: IsHeader + Clone + Sync + Send> ChainStore<H> for InMemConsensusStore<H> {
    fn load_header(&self, hash: &Hash<32>) -> Option<H> {
        self.headers.get(hash).cloned()
    }

    fn store_header(&mut self, hash: &Hash<32>, header: &H) -> Result<(), StoreError> {
        self.headers.insert(*hash, header.clone());
        Ok(())
    }

    fn get_nonces(&self, header: &Hash<32>) -> Option<Nonces> {
        self.nonces.get(header).cloned()
    }

    fn put_nonces(&mut self, header: &Hash<32>, nonces: &Nonces) -> Result<(), StoreError> {
        self.nonces.insert(*header, nonces.clone());
        Ok(())
    }

    fn era_history(&self) -> &amaru_kernel::EraHistory {
        &self.era_history
    }

    fn load_block(&self, hash: &Hash<32>) -> Result<RawBlock, StoreError> {
        self.blocks
            .get(hash)
            .cloned()
            .ok_or(StoreError::NotFound { hash: *hash })
    }

    fn store_block(&mut self, hash: &Hash<32>, block: &RawBlock) -> Result<(), StoreError> {
        self.blocks.insert(*hash, block.clone());
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use amaru_kernel::network::NetworkName;
    use amaru_ouroboros_traits::is_header::fake::FakeHeader;
    use rand::{rngs::StdRng, RngCore, SeedableRng};
    use std::fs::create_dir;

    /// FIXME: already exists in chain_selection test module
    pub fn random_bytes(arg: u32) -> Vec<u8> {
        let mut rng = StdRng::from_os_rng();
        let mut buffer = vec![0; arg as usize];
        rng.fill_bytes(&mut buffer);
        buffer
    }

    fn initialise_test_store() -> RocksDBStore {
        let tempdir = tempfile::tempdir().unwrap();
        let basedir = tempdir.path().join("rocksdb_chain_store");
        let era_history: &EraHistory = NetworkName::Testnet(42).into();

        create_dir(&basedir).unwrap();
        RocksDBStore::new(&basedir, era_history).expect("fail to initialise RocksDB")
    }

    #[test]
    fn rocksdb_chain_store_can_get_header_it_puts() {
        let mut store = initialise_test_store();

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
        let mut store = initialise_test_store();

        let hash: Hash<32> = random_bytes(32).as_slice().into();
        let block = vec![1; 64];

        <RocksDBStore as ChainStore<FakeHeader>>::store_block(&mut store, &hash, &block).unwrap();
        let block2 = <RocksDBStore as ChainStore<FakeHeader>>::load_block(&store, &hash).unwrap();
        assert_eq!(block, block2);
    }

    #[test]
    fn rocksdb_chain_store_returns_not_found_for_nonexistent_block() {
        let store = initialise_test_store();

        let nonexistent_hash: Hash<32> = random_bytes(32).as_slice().into();

        let result =
            <RocksDBStore as ChainStore<FakeHeader>>::load_block(&store, &nonexistent_hash);

        assert_eq!(
            Err(StoreError::NotFound {
                hash: nonexistent_hash
            }),
            result
        );
    }

    #[test]
    fn in_mem_chain_store_can_get_header_it_puts() {
        let mut store: InMemConsensusStore<FakeHeader> = InMemConsensusStore::new();

        let header = FakeHeader {
            block_number: 1,
            slot: 0,
            parent: None,
            body_hash: random_bytes(32).as_slice().into(),
        };

        store.store_header(&header.hash(), &header).unwrap();

        let header2 = store
            .load_header(&header.hash())
            .expect("Header should be found");

        assert_eq!(header, header2);
    }
}
