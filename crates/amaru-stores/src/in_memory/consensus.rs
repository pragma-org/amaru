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
use amaru_kernel::network::NetworkName;
use amaru_kernel::{Hash, ORIGIN_HASH, RawBlock};
use amaru_ouroboros_traits::{IsHeader, Nonces};
use amaru_slot_arithmetic::EraHistory;
use std::collections::BTreeMap;
use std::sync::Mutex;
use tracing::error;

pub struct InMemConsensusStore<H> {
    inner: Mutex<InMemConsensusStoreInner<H>>,
}

impl<H> Default for InMemConsensusStore<H> {
    fn default() -> Self {
        Self::new()
    }
}

impl<H> InMemConsensusStore<H> {
    pub fn new() -> InMemConsensusStore<H> {
        InMemConsensusStore {
            inner: Mutex::new(InMemConsensusStoreInner::new()),
        }
    }
}

struct InMemConsensusStoreInner<H> {
    nonces: BTreeMap<Hash<32>, Nonces>,
    headers: BTreeMap<Hash<32>, H>,
    parent_child_relationship: BTreeMap<Hash<32>, Vec<Hash<32>>>,
    root: Hash<32>,
    blocks: BTreeMap<Hash<32>, RawBlock>,
}

impl<H> Default for InMemConsensusStoreInner<H> {
    fn default() -> Self {
        Self::new()
    }
}

impl<H> InMemConsensusStoreInner<H> {
    fn new() -> InMemConsensusStoreInner<H> {
        InMemConsensusStoreInner {
            nonces: BTreeMap::new(),
            headers: BTreeMap::new(),
            parent_child_relationship: BTreeMap::new(),
            root: ORIGIN_HASH,
            blocks: BTreeMap::new(),
        }
    }
}

impl<H: IsHeader + Clone + Send + Sync + Clone> ReadOnlyChainStore<H> for InMemConsensusStore<H> {
    #[expect(clippy::unwrap_used)]
    fn load_header(&self, hash: &Hash<32>) -> Option<H> {
        let inner = self.inner.lock().unwrap();
        inner.headers.get(hash).cloned()
    }

    #[expect(clippy::unwrap_used)]
    fn load_headers(&self) -> Vec<H> {
        let inner = self.inner.lock().unwrap();
        inner.headers.values().cloned().collect()
    }

    #[expect(clippy::unwrap_used)]
    fn count_headers(&self) -> usize {
        let inner = self.inner.lock().unwrap();
        inner.headers.len()
    }

    #[expect(clippy::unwrap_used)]
    fn get_nonces(&self, header: &Hash<32>) -> Option<Nonces> {
        let inner = self.inner.lock().unwrap();
        inner.nonces.get(header).cloned().or_else(|| {
            error!("failed to find nonce {}", header);
            for (key, value) in inner.headers.iter() {
                error!("{:?}: {:?}", key, value.hash());
            }
            None
        })
    }

    fn load_block(&self, _hash: &Hash<32>) -> Result<RawBlock, StoreError> {
        unimplemented!("load_block is not implemented for InMemConsensusStore")
    }

    #[expect(clippy::unwrap_used)]
    fn has_header(&self, hash: &Hash<32>) -> bool {
        let inner = self.inner.lock().unwrap();
        inner.headers.contains_key(hash)
    }

    #[expect(clippy::unwrap_used)]
    fn get_children(&self, hash: &Hash<32>) -> Vec<Hash<32>> {
        let inner = self.inner.lock().unwrap();
        inner
            .parent_child_relationship
            .get(hash)
            .cloned()
            .unwrap_or_default()
    }

    #[expect(clippy::unwrap_used)]
    fn get_root_hash(&self) -> Hash<32> {
        let inner = self.inner.lock().unwrap();
        inner.root
    }
}

impl<H: IsHeader + Send + Sync + Clone> ChainStore<H> for InMemConsensusStore<H> {
    #[expect(clippy::unwrap_used)]
    fn store_header(&self, hash: &Hash<32>, header: &H) -> Result<(), StoreError> {
        let mut inner = self.inner.lock().unwrap();
        inner.headers.insert(*hash, header.clone());
        if let Some(parent) = header.parent() {
            inner
                .parent_child_relationship
                .entry(parent)
                .or_default()
                .push(*hash);
        }
        Ok(())
    }

    #[expect(clippy::unwrap_used)]
    fn put_nonces(&self, header: &Hash<32>, nonces: &Nonces) -> Result<(), StoreError> {
        let mut inner = self.inner.lock().unwrap();
        inner.nonces.insert(*header, nonces.clone());
        Ok(())
    }

    fn era_history(&self) -> &EraHistory {
        NetworkName::Testnet(42).into()
    }

    #[expect(clippy::unwrap_used)]
    fn store_block(&self, hash: &Hash<32>, block: &RawBlock) -> Result<(), StoreError> {
        let mut inner = self.inner.lock().unwrap();
        inner.blocks.insert(*hash, (*block).clone());
        Ok(())
    }

    #[expect(clippy::unwrap_used)]
    fn remove_header(&self, hash: &Hash<32>) -> Result<(), StoreError> {
        let mut inner = self.inner.lock().unwrap();
        inner.headers.remove(hash);
        inner.parent_child_relationship.remove(hash);
        Ok(())
    }

    #[expect(clippy::unwrap_used)]
    fn set_root_hash(&self, hash: &Hash<32>) -> Result<(), StoreError> {
        let mut inner = self.inner.lock().unwrap();
        inner.root = *hash;
        Ok(())
    }
}
