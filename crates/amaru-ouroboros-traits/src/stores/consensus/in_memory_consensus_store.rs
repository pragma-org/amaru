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

use crate::{ChainStore, ReadOnlyChainStore, StoreError};
use crate::{IsHeader, Nonces};
use amaru_kernel::network::NetworkName;
use amaru_kernel::{HEADER_HASH_SIZE, Hash, ORIGIN_HASH, RawBlock};
use amaru_slot_arithmetic::EraHistory;
use std::collections::BTreeMap;
use std::sync::Mutex;

/// An in-memory implementation of a ChainStore used by the consensus stages.
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
    anchor: Hash<32>,
    best_chain: Hash<32>,
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
            anchor: ORIGIN_HASH,
            best_chain: ORIGIN_HASH,
            blocks: BTreeMap::new(),
        }
    }
}

impl<H: IsHeader + Clone + Send + Sync + Clone + 'static> ReadOnlyChainStore<H>
    for InMemConsensusStore<H>
{
    #[expect(clippy::unwrap_used)]
    fn load_header(&self, hash: &Hash<32>) -> Option<H> {
        let inner = self.inner.lock().unwrap();
        inner.headers.get(hash).cloned()
    }

    #[expect(clippy::unwrap_used)]
    fn load_headers(&self) -> Box<dyn Iterator<Item = H>> {
        let inner = self.inner.lock().unwrap();
        Box::new(
            inner
                .headers
                .values()
                .cloned()
                .collect::<Vec<H>>()
                .into_iter(),
        )
    }

    #[expect(clippy::unwrap_used)]
    fn load_nonces(&self) -> Box<dyn Iterator<Item = (Hash<32>, Nonces)> + '_> {
        let inner = self.inner.lock().unwrap();
        Box::new(
            inner
                .nonces
                .iter()
                .map(|(h, n)| (*h, n.clone()))
                .collect::<Vec<_>>()
                .into_iter(),
        )
    }

    #[expect(clippy::unwrap_used)]
    fn load_blocks(&self) -> Box<dyn Iterator<Item = (Hash<32>, RawBlock)> + '_> {
        let inner = self.inner.lock().unwrap();
        Box::new(
            inner
                .blocks
                .iter()
                .map(|(h, b)| (*h, b.clone()))
                .collect::<Vec<_>>()
                .into_iter(),
        )
    }

    #[expect(clippy::unwrap_used)]
    fn load_parents_children(
        &self,
    ) -> Box<dyn Iterator<Item = (Hash<HEADER_HASH_SIZE>, Vec<Hash<HEADER_HASH_SIZE>>)> + '_> {
        let inner = self.inner.lock().unwrap();
        Box::new(
            inner
                .parent_child_relationship
                .iter()
                .map(|(k, v)| (*k, v.to_vec()))
                .collect::<Vec<(Hash<HEADER_HASH_SIZE>, Vec<Hash<HEADER_HASH_SIZE>>)>>()
                .into_iter(),
        )
    }

    #[expect(clippy::unwrap_used)]
    fn get_nonces(&self, header: &Hash<32>) -> Option<Nonces> {
        let inner = self.inner.lock().unwrap();
        inner.nonces.get(header).cloned()
    }

    #[expect(clippy::unwrap_used)]
    fn load_block(&self, hash: &Hash<32>) -> Result<RawBlock, StoreError> {
        let inner = self.inner.lock().unwrap();
        inner
            .blocks
            .get(hash)
            .cloned()
            .ok_or(StoreError::NotFound { hash: *hash })
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
    fn get_anchor_hash(&self) -> Hash<32> {
        let inner = self.inner.lock().unwrap();
        inner.anchor
    }

    #[expect(clippy::unwrap_used)]
    fn get_best_chain_hash(&self) -> Hash<32> {
        let inner = self.inner.lock().unwrap();
        inner.best_chain
    }
}

impl<H: IsHeader + Send + Sync + Clone + 'static> ChainStore<H> for InMemConsensusStore<H> {
    #[expect(clippy::unwrap_used)]
    fn store_header(&self, header: &H) -> Result<(), StoreError> {
        let hash = header.hash();
        let mut inner = self.inner.lock().unwrap();
        inner.headers.insert(hash, header.clone());
        if let Some(parent) = header.parent() {
            let children = inner.parent_child_relationship.entry(parent).or_default();
            if !children.contains(&hash) {
                children.push(hash);
            }
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
        NetworkName::Preprod.into()
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
        // unlink from parent's children list if present
        if let Some(parent) = inner.headers.get(hash).and_then(|h| h.parent())
            && let Some(children) = inner.parent_child_relationship.get_mut(&parent)
        {
            if let Some(pos) = children.iter().position(|h| h == hash) {
                children.swap_remove(pos);
            }
            if children.is_empty() {
                inner.parent_child_relationship.remove(&parent);
            }
        }

        // remove this node's children list
        inner.parent_child_relationship.remove(hash);
        // finally remove the header
        inner.headers.remove(hash);
        Ok(())
    }

    #[expect(clippy::unwrap_used)]
    fn set_anchor_hash(&self, hash: &Hash<32>) -> Result<(), StoreError> {
        let mut inner = self.inner.lock().unwrap();
        inner.anchor = *hash;
        Ok(())
    }

    #[expect(clippy::unwrap_used)]
    fn set_best_chain_hash(&self, hash: &Hash<32>) -> Result<(), StoreError> {
        let mut inner = self.inner.lock().unwrap();
        inner.best_chain = *hash;
        Ok(())
    }

    #[expect(clippy::unwrap_used)]
    fn update_best_chain(&self, anchor: &Hash<32>, tip: &Hash<32>) -> Result<(), StoreError> {
        let mut inner = self.inner.lock().unwrap();
        inner.anchor = *anchor;
        inner.best_chain = *tip;
        Ok(())
    }
}
