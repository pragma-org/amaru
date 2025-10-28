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

use crate::{ChainStore, Nonces, ReadOnlyChainStore, StoreError};
use amaru_kernel::{HeaderHash, IsHeader, ORIGIN_HASH, Point, RawBlock};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

/// An in-memory implementation of a ChainStore used by the consensus stages.
#[derive(Clone)]
pub struct InMemConsensusStore<H> {
    inner: Arc<Mutex<InMemConsensusStoreInner<H>>>,
}

impl<H> Default for InMemConsensusStore<H> {
    fn default() -> Self {
        Self::new()
    }
}

impl<H> InMemConsensusStore<H> {
    pub fn new() -> InMemConsensusStore<H> {
        InMemConsensusStore {
            inner: Arc::new(Mutex::new(InMemConsensusStoreInner::new())),
        }
    }
}

struct InMemConsensusStoreInner<H> {
    nonces: BTreeMap<HeaderHash, Nonces>,
    headers: BTreeMap<HeaderHash, H>,
    parent_child_relationship: BTreeMap<HeaderHash, Vec<HeaderHash>>,
    anchor: HeaderHash,
    best_chain: HeaderHash,
    blocks: BTreeMap<HeaderHash, RawBlock>,
    chain: Vec<Point>,
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
            chain: Vec::new(),
        }
    }
}

impl<H: IsHeader + Clone + Send + Sync + 'static> ReadOnlyChainStore<H> for InMemConsensusStore<H> {
    #[expect(clippy::unwrap_used)]
    fn load_header(&self, hash: &HeaderHash) -> Option<H> {
        let inner = self.inner.lock().unwrap();
        inner.headers.get(hash).cloned()
    }

    #[expect(clippy::unwrap_used)]
    fn get_nonces(&self, header: &HeaderHash) -> Option<Nonces> {
        let inner = self.inner.lock().unwrap();
        inner.nonces.get(header).cloned()
    }

    #[expect(clippy::unwrap_used)]
    fn load_block(&self, hash: &HeaderHash) -> Result<RawBlock, StoreError> {
        let inner = self.inner.lock().unwrap();
        inner
            .blocks
            .get(hash)
            .cloned()
            .ok_or(StoreError::NotFound { hash: *hash })
    }

    #[expect(clippy::unwrap_used)]
    fn has_header(&self, hash: &HeaderHash) -> bool {
        let inner = self.inner.lock().unwrap();
        inner.headers.contains_key(hash)
    }

    #[expect(clippy::unwrap_used)]
    fn get_children(&self, hash: &HeaderHash) -> Vec<HeaderHash> {
        let inner = self.inner.lock().unwrap();
        inner
            .parent_child_relationship
            .get(hash)
            .cloned()
            .unwrap_or_default()
    }

    #[expect(clippy::unwrap_used)]
    fn get_anchor_hash(&self) -> HeaderHash {
        let inner = self.inner.lock().unwrap();
        inner.anchor
    }

    #[expect(clippy::unwrap_used)]
    fn get_best_chain_hash(&self) -> HeaderHash {
        let inner = self.inner.lock().unwrap();
        inner.best_chain
    }

    #[expect(clippy::unwrap_used)]
    fn load_from_best_chain(&self, point: &Point) -> Option<HeaderHash> {
        let inner = self.inner.lock().unwrap();
        inner.chain.iter().find(|p| *p == point).map(|p| p.hash())
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
    fn put_nonces(&self, header: &HeaderHash, nonces: &Nonces) -> Result<(), StoreError> {
        let mut inner = self.inner.lock().unwrap();
        inner.nonces.insert(*header, nonces.clone());
        Ok(())
    }

    #[expect(clippy::unwrap_used)]
    fn store_block(&self, hash: &HeaderHash, block: &RawBlock) -> Result<(), StoreError> {
        let mut inner = self.inner.lock().unwrap();
        inner.blocks.insert(*hash, (*block).clone());
        Ok(())
    }

    #[expect(clippy::unwrap_used)]
    fn set_anchor_hash(&self, hash: &HeaderHash) -> Result<(), StoreError> {
        let mut inner = self.inner.lock().unwrap();
        inner.anchor = *hash;
        Ok(())
    }

    #[expect(clippy::unwrap_used)]
    fn set_best_chain_hash(&self, hash: &HeaderHash) -> Result<(), StoreError> {
        let mut inner = self.inner.lock().unwrap();
        inner.best_chain = *hash;
        Ok(())
    }

    #[expect(clippy::unwrap_used)]
    fn roll_forward_chain(&self, point: &Point) -> Result<(), StoreError> {
        let mut inner = self.inner.lock().unwrap();
        inner.chain.push(point.clone());
        Ok(())
    }

    #[expect(clippy::unwrap_used)]
    fn rollback_chain(&self, point: &Point) -> Result<usize, StoreError> {
        let mut inner = self.inner.lock().unwrap();
        if let Some(pos) = inner.chain.iter().rposition(|p| p == point) {
            let removed = inner.chain.len().saturating_sub(pos + 1);
            inner.chain.truncate(pos + 1); // keep the rollback point
            Ok(removed)
        } else {
            Err(StoreError::ReadError {
                error: format!(
                    "Cannot roll back chain to point {:?} as it does not exist on the best chain",
                    point
                ),
            })
        }
    }
}
