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

use std::sync::Arc;

use amaru_kernel::{BlockHeader, HeaderHash, Point, RawBlock};
use parking_lot::Mutex;

use crate::{BaseReadChainStore, ChainStore, Nonces, ReadChainStore, StoreError, WriteChainStore};

/// A chain store that wraps a `dyn ChainStore` and allows overriding any method
/// with a supplied function. When an override is installed, it receives a reference
/// to the underlying store, all method arguments, and computes the return value.
/// Non-overridden methods delegate to the underlying store.
///
/// Overrides use `FnMut` and are stored in a `parking_lot::Mutex` to allow mutation.
pub struct OverridingChainStore {
    inner: Arc<dyn ChainStore>,
    overrides: Mutex<Overrides>,
}

/// Optional method overrides for [`OverridingChainStore`].
/// Each override receives a reference to the underlying store and the method arguments.
/// Overrides are stored in a mutex because they use `FnMut`.
#[expect(clippy::type_complexity)]
#[derive(Default)]
struct Overrides {
    load_header: Option<Box<dyn FnMut(&dyn BaseReadChainStore, &HeaderHash) -> Option<BlockHeader> + Send>>,
    load_header_with_validity:
        Option<Box<dyn FnMut(&dyn BaseReadChainStore, &HeaderHash) -> Option<(BlockHeader, Option<bool>)> + Send>>,
    get_children: Option<Box<dyn FnMut(&dyn BaseReadChainStore, &HeaderHash) -> Vec<HeaderHash> + Send>>,
    get_anchor_hash: Option<Box<dyn FnMut(&dyn BaseReadChainStore) -> HeaderHash + Send>>,
    get_best_chain_hash: Option<Box<dyn FnMut(&dyn BaseReadChainStore) -> HeaderHash + Send>>,
    load_from_best_chain: Option<Box<dyn FnMut(&dyn BaseReadChainStore, &Point) -> Option<HeaderHash> + Send>>,
    next_best_chain: Option<Box<dyn FnMut(&dyn BaseReadChainStore, &Point) -> Option<Point> + Send>>,
    load_block:
        Option<Box<dyn FnMut(&dyn BaseReadChainStore, &HeaderHash) -> Result<Option<RawBlock>, StoreError> + Send>>,
    has_block: Option<Box<dyn FnMut(&dyn BaseReadChainStore, &HeaderHash) -> Result<bool, StoreError> + Send>>,
    get_nonces: Option<Box<dyn FnMut(&dyn BaseReadChainStore, &HeaderHash) -> Option<Nonces> + Send>>,
    has_header: Option<Box<dyn FnMut(&dyn BaseReadChainStore, &HeaderHash) -> bool + Send>>,
    store_header: Option<Box<dyn FnMut(&dyn ChainStore, &BlockHeader) -> Result<(), StoreError> + Send>>,
    set_anchor_hash: Option<Box<dyn FnMut(&dyn ChainStore, &HeaderHash) -> Result<(), StoreError> + Send>>,
    set_best_chain_hash: Option<Box<dyn FnMut(&dyn ChainStore, &HeaderHash) -> Result<(), StoreError> + Send>>,
    store_block: Option<Box<dyn FnMut(&dyn ChainStore, &HeaderHash, &RawBlock) -> Result<(), StoreError> + Send>>,
    set_block_valid: Option<Box<dyn FnMut(&dyn ChainStore, &HeaderHash, bool) -> Result<(), StoreError> + Send>>,
    put_nonces: Option<Box<dyn FnMut(&dyn ChainStore, &HeaderHash, &Nonces) -> Result<(), StoreError> + Send>>,
    switch_to_fork: Option<Box<dyn FnMut(&dyn ChainStore, &Point, &[Point]) -> Result<(), StoreError> + Send>>,
    roll_forward_chain: Option<Box<dyn FnMut(&dyn ChainStore, &Point) -> Result<(), StoreError> + Send>>,
}

struct OverridingChainStoreSnapshot<'a> {
    parent: &'a OverridingChainStore,
    inner: Box<dyn BaseReadChainStore + 'a>,
}

impl OverridingChainStore {
    /// Create a new builder for an overriding chain store wrapping the given store.
    pub fn builder(inner: Arc<dyn ChainStore>) -> OverridingChainStoreBuilder {
        OverridingChainStoreBuilder { inner, overrides: Overrides::default() }
    }
}

/// Builder for [`OverridingChainStore`] that accepts override functions via `impl FnMut`.
pub struct OverridingChainStoreBuilder {
    inner: Arc<dyn ChainStore>,
    overrides: Overrides,
}

impl OverridingChainStoreBuilder {
    pub fn with_load_header<F>(mut self, f: F) -> Self
    where
        F: FnMut(&dyn BaseReadChainStore, &HeaderHash) -> Option<BlockHeader> + Send + 'static,
    {
        self.overrides.load_header = Some(Box::new(f));
        self
    }

    pub fn with_load_header_with_validity<F>(mut self, f: F) -> Self
    where
        F: FnMut(&dyn BaseReadChainStore, &HeaderHash) -> Option<(BlockHeader, Option<bool>)> + Send + 'static,
    {
        self.overrides.load_header_with_validity = Some(Box::new(f));
        self
    }

    pub fn with_get_children<F>(mut self, f: F) -> Self
    where
        F: FnMut(&dyn BaseReadChainStore, &HeaderHash) -> Vec<HeaderHash> + Send + 'static,
    {
        self.overrides.get_children = Some(Box::new(f));
        self
    }

    pub fn with_get_anchor_hash<F>(mut self, f: F) -> Self
    where
        F: FnMut(&dyn BaseReadChainStore) -> HeaderHash + Send + 'static,
    {
        self.overrides.get_anchor_hash = Some(Box::new(f));
        self
    }

    pub fn with_get_best_chain_hash<F>(mut self, f: F) -> Self
    where
        F: FnMut(&dyn BaseReadChainStore) -> HeaderHash + Send + 'static,
    {
        self.overrides.get_best_chain_hash = Some(Box::new(f));
        self
    }

    pub fn with_load_from_best_chain<F>(mut self, f: F) -> Self
    where
        F: FnMut(&dyn BaseReadChainStore, &Point) -> Option<HeaderHash> + Send + 'static,
    {
        self.overrides.load_from_best_chain = Some(Box::new(f));
        self
    }

    pub fn with_next_best_chain<F>(mut self, f: F) -> Self
    where
        F: FnMut(&dyn BaseReadChainStore, &Point) -> Option<Point> + Send + 'static,
    {
        self.overrides.next_best_chain = Some(Box::new(f));
        self
    }

    pub fn with_load_block<F>(mut self, f: F) -> Self
    where
        F: FnMut(&dyn BaseReadChainStore, &HeaderHash) -> Result<Option<RawBlock>, StoreError> + Send + 'static,
    {
        self.overrides.load_block = Some(Box::new(f));
        self
    }

    pub fn with_has_block<F>(mut self, f: F) -> Self
    where
        F: FnMut(&dyn BaseReadChainStore, &HeaderHash) -> Result<bool, StoreError> + Send + 'static,
    {
        self.overrides.has_block = Some(Box::new(f));
        self
    }

    pub fn with_get_nonces<F>(mut self, f: F) -> Self
    where
        F: FnMut(&dyn BaseReadChainStore, &HeaderHash) -> Option<Nonces> + Send + 'static,
    {
        self.overrides.get_nonces = Some(Box::new(f));
        self
    }

    pub fn with_has_header<F>(mut self, f: F) -> Self
    where
        F: FnMut(&dyn BaseReadChainStore, &HeaderHash) -> bool + Send + 'static,
    {
        self.overrides.has_header = Some(Box::new(f));
        self
    }

    pub fn with_store_header<F>(mut self, f: F) -> Self
    where
        F: FnMut(&dyn ChainStore, &BlockHeader) -> Result<(), StoreError> + Send + 'static,
    {
        self.overrides.store_header = Some(Box::new(f));
        self
    }

    pub fn with_set_anchor_hash<F>(mut self, f: F) -> Self
    where
        F: FnMut(&dyn ChainStore, &HeaderHash) -> Result<(), StoreError> + Send + 'static,
    {
        self.overrides.set_anchor_hash = Some(Box::new(f));
        self
    }

    pub fn with_set_best_chain_hash<F>(mut self, f: F) -> Self
    where
        F: FnMut(&dyn ChainStore, &HeaderHash) -> Result<(), StoreError> + Send + 'static,
    {
        self.overrides.set_best_chain_hash = Some(Box::new(f));
        self
    }

    pub fn with_store_block<F>(mut self, f: F) -> Self
    where
        F: FnMut(&dyn ChainStore, &HeaderHash, &RawBlock) -> Result<(), StoreError> + Send + 'static,
    {
        self.overrides.store_block = Some(Box::new(f));
        self
    }

    pub fn with_set_block_valid<F>(mut self, f: F) -> Self
    where
        F: FnMut(&dyn ChainStore, &HeaderHash, bool) -> Result<(), StoreError> + Send + 'static,
    {
        self.overrides.set_block_valid = Some(Box::new(f));
        self
    }

    pub fn with_put_nonces<F>(mut self, f: F) -> Self
    where
        F: FnMut(&dyn ChainStore, &HeaderHash, &Nonces) -> Result<(), StoreError> + Send + 'static,
    {
        self.overrides.put_nonces = Some(Box::new(f));
        self
    }

    pub fn with_switch_to_fork<F>(mut self, f: F) -> Self
    where
        F: FnMut(&dyn ChainStore, &Point, &[Point]) -> Result<(), StoreError> + Send + 'static,
    {
        self.overrides.switch_to_fork = Some(Box::new(f));
        self
    }

    pub fn with_roll_forward_chain<F>(mut self, f: F) -> Self
    where
        F: FnMut(&dyn ChainStore, &Point) -> Result<(), StoreError> + Send + 'static,
    {
        self.overrides.roll_forward_chain = Some(Box::new(f));
        self
    }

    pub fn build(self) -> OverridingChainStore {
        OverridingChainStore { inner: self.inner, overrides: Mutex::new(self.overrides) }
    }
}

impl BaseReadChainStore for OverridingChainStore {
    fn load_header(&self, hash: &HeaderHash) -> Option<BlockHeader> {
        let mut overrides = self.overrides.lock();
        match &mut overrides.load_header {
            Some(f) => f(self.inner.as_ref(), hash),
            None => self.inner.load_header(hash),
        }
    }

    fn load_header_with_validity(&self, hash: &HeaderHash) -> Option<(BlockHeader, Option<bool>)> {
        let mut overrides = self.overrides.lock();
        match &mut overrides.load_header_with_validity {
            Some(f) => f(self.inner.as_ref(), hash),
            None => self.inner.load_header_with_validity(hash),
        }
    }

    fn get_children(&self, hash: &HeaderHash) -> Vec<HeaderHash> {
        let mut overrides = self.overrides.lock();
        match &mut overrides.get_children {
            Some(f) => f(self.inner.as_ref(), hash),
            None => self.inner.get_children(hash),
        }
    }

    fn get_anchor_hash(&self) -> HeaderHash {
        let mut overrides = self.overrides.lock();
        match &mut overrides.get_anchor_hash {
            Some(f) => f(self.inner.as_ref()),
            None => self.inner.get_anchor_hash(),
        }
    }

    fn get_best_chain_hash(&self) -> HeaderHash {
        let mut overrides = self.overrides.lock();
        match &mut overrides.get_best_chain_hash {
            Some(f) => f(self.inner.as_ref()),
            None => self.inner.get_best_chain_hash(),
        }
    }

    fn load_from_best_chain(&self, point: &Point) -> Option<HeaderHash> {
        let mut overrides = self.overrides.lock();
        match &mut overrides.load_from_best_chain {
            Some(f) => f(self.inner.as_ref(), point),
            None => self.inner.load_from_best_chain(point),
        }
    }

    fn next_best_chain(&self, point: &Point) -> Option<Point> {
        let mut overrides = self.overrides.lock();
        match &mut overrides.next_best_chain {
            Some(f) => f(self.inner.as_ref(), point),
            None => self.inner.next_best_chain(point),
        }
    }

    fn load_block(&self, hash: &HeaderHash) -> Result<Option<RawBlock>, StoreError> {
        let mut overrides = self.overrides.lock();
        match &mut overrides.load_block {
            Some(f) => f(self.inner.as_ref(), hash),
            None => self.inner.load_block(hash),
        }
    }

    fn has_block(&self, hash: &HeaderHash) -> Result<bool, StoreError> {
        let mut overrides = self.overrides.lock();
        match &mut overrides.has_block {
            Some(f) => f(self.inner.as_ref(), hash),
            None => self.inner.has_block(hash),
        }
    }

    fn get_nonces(&self, header: &HeaderHash) -> Option<Nonces> {
        let mut overrides = self.overrides.lock();
        match &mut overrides.get_nonces {
            Some(f) => f(self.inner.as_ref(), header),
            None => self.inner.get_nonces(header),
        }
    }

    fn has_header(&self, hash: &HeaderHash) -> bool {
        let mut overrides = self.overrides.lock();
        match &mut overrides.has_header {
            Some(f) => f(self.inner.as_ref(), hash),
            None => self.inner.has_header(hash),
        }
    }
}

impl ReadChainStore for OverridingChainStore {
    fn snapshot(&self) -> Box<dyn BaseReadChainStore + '_> {
        Box::new(OverridingChainStoreSnapshot { parent: self, inner: self.inner.snapshot() })
    }
}

impl BaseReadChainStore for OverridingChainStoreSnapshot<'_> {
    fn load_header(&self, hash: &HeaderHash) -> Option<BlockHeader> {
        let mut overrides = self.parent.overrides.lock();
        match &mut overrides.load_header {
            Some(f) => f(self.inner.as_ref(), hash),
            None => self.inner.load_header(hash),
        }
    }

    fn load_header_with_validity(&self, hash: &HeaderHash) -> Option<(BlockHeader, Option<bool>)> {
        let mut overrides = self.parent.overrides.lock();
        match &mut overrides.load_header_with_validity {
            Some(f) => f(self.inner.as_ref(), hash),
            None => self.inner.load_header_with_validity(hash),
        }
    }

    fn get_children(&self, hash: &HeaderHash) -> Vec<HeaderHash> {
        let mut overrides = self.parent.overrides.lock();
        match &mut overrides.get_children {
            Some(f) => f(self.inner.as_ref(), hash),
            None => self.inner.get_children(hash),
        }
    }

    fn get_anchor_hash(&self) -> HeaderHash {
        let mut overrides = self.parent.overrides.lock();
        match &mut overrides.get_anchor_hash {
            Some(f) => f(self.inner.as_ref()),
            None => self.inner.get_anchor_hash(),
        }
    }

    fn get_best_chain_hash(&self) -> HeaderHash {
        let mut overrides = self.parent.overrides.lock();
        match &mut overrides.get_best_chain_hash {
            Some(f) => f(self.inner.as_ref()),
            None => self.inner.get_best_chain_hash(),
        }
    }

    fn load_from_best_chain(&self, point: &Point) -> Option<HeaderHash> {
        let mut overrides = self.parent.overrides.lock();
        match &mut overrides.load_from_best_chain {
            Some(f) => f(self.inner.as_ref(), point),
            None => self.inner.load_from_best_chain(point),
        }
    }

    fn next_best_chain(&self, point: &Point) -> Option<Point> {
        let mut overrides = self.parent.overrides.lock();
        match &mut overrides.next_best_chain {
            Some(f) => f(self.inner.as_ref(), point),
            None => self.inner.next_best_chain(point),
        }
    }

    fn load_block(&self, hash: &HeaderHash) -> Result<Option<RawBlock>, StoreError> {
        let mut overrides = self.parent.overrides.lock();
        match &mut overrides.load_block {
            Some(f) => f(self.inner.as_ref(), hash),
            None => self.inner.load_block(hash),
        }
    }

    fn has_block(&self, hash: &HeaderHash) -> Result<bool, StoreError> {
        let mut overrides = self.parent.overrides.lock();
        match &mut overrides.has_block {
            Some(f) => f(self.inner.as_ref(), hash),
            None => self.inner.has_block(hash),
        }
    }

    fn get_nonces(&self, header: &HeaderHash) -> Option<Nonces> {
        let mut overrides = self.parent.overrides.lock();
        match &mut overrides.get_nonces {
            Some(f) => f(self.inner.as_ref(), header),
            None => self.inner.get_nonces(header),
        }
    }

    fn has_header(&self, hash: &HeaderHash) -> bool {
        let mut overrides = self.parent.overrides.lock();
        match &mut overrides.has_header {
            Some(f) => f(self.inner.as_ref(), hash),
            None => self.inner.has_header(hash),
        }
    }
}

impl WriteChainStore for OverridingChainStore {
    fn store_header(&self, header: &BlockHeader) -> Result<(), StoreError> {
        let mut overrides = self.overrides.lock();
        match &mut overrides.store_header {
            Some(f) => f(self.inner.as_ref(), header),
            None => self.inner.store_header(header),
        }
    }

    fn set_anchor_hash(&self, hash: &HeaderHash) -> Result<(), StoreError> {
        let mut overrides = self.overrides.lock();
        match &mut overrides.set_anchor_hash {
            Some(f) => f(self.inner.as_ref(), hash),
            None => self.inner.set_anchor_hash(hash),
        }
    }

    fn set_best_chain_hash(&self, hash: &HeaderHash) -> Result<(), StoreError> {
        let mut overrides = self.overrides.lock();
        match &mut overrides.set_best_chain_hash {
            Some(f) => f(self.inner.as_ref(), hash),
            None => self.inner.set_best_chain_hash(hash),
        }
    }

    fn store_block(&self, hash: &HeaderHash, block: &RawBlock) -> Result<(), StoreError> {
        let mut overrides = self.overrides.lock();
        match &mut overrides.store_block {
            Some(f) => f(self.inner.as_ref(), hash, block),
            None => self.inner.store_block(hash, block),
        }
    }

    fn set_block_valid(&self, hash: &HeaderHash, valid: bool) -> Result<(), StoreError> {
        let mut overrides = self.overrides.lock();
        match &mut overrides.set_block_valid {
            Some(f) => f(self.inner.as_ref(), hash, valid),
            None => self.inner.set_block_valid(hash, valid),
        }
    }

    fn put_nonces(&self, header: &HeaderHash, nonces: &Nonces) -> Result<(), StoreError> {
        let mut overrides = self.overrides.lock();
        match &mut overrides.put_nonces {
            Some(f) => f(self.inner.as_ref(), header, nonces),
            None => self.inner.put_nonces(header, nonces),
        }
    }

    fn switch_to_fork(&self, fork_point: &Point, forward_points: &[Point]) -> Result<(), StoreError> {
        let mut overrides = self.overrides.lock();
        match &mut overrides.switch_to_fork {
            Some(f) => f(self.inner.as_ref(), fork_point, forward_points),
            None => self.inner.switch_to_fork(fork_point, forward_points),
        }
    }

    fn roll_forward_chain(&self, point: &Point) -> Result<(), StoreError> {
        let mut overrides = self.overrides.lock();
        match &mut overrides.roll_forward_chain {
            Some(f) => f(self.inner.as_ref(), point),
            None => self.inner.roll_forward_chain(point),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use amaru_kernel::{BlockHeader, IsHeader, make_header};

    use super::*;
    use crate::{FindAncestorOnBestChainResult, in_memory_chain_store::InMemoryChainStore};

    #[test]
    fn snapshot_respects_read_overrides_used_by_default_helpers() {
        let inner: Arc<dyn ChainStore> = Arc::new(InMemoryChainStore::new());
        let chain = create_best_chain(inner.as_ref(), 3);
        let hidden_point = chain[1].point();
        let hidden_hash = chain[1].hash();
        let store = OverridingChainStore::builder(inner)
            .with_load_from_best_chain(
                move |_store, point| {
                    if point == &hidden_point { None } else { Some(point.hash()) }
                },
            )
            .build();

        let Ok(FindAncestorOnBestChainResult::Found { fork_point, forward_points }) =
            store.find_ancestor_on_best_chain(hidden_hash)
        else {
            panic!("the fork point must be found")
        };
        assert_eq!(fork_point, chain[0].point());
        assert_eq!(forward_points.as_ref(), &[hidden_point]);
    }

    #[test]
    fn snapshot_read_overrides_see_frozen_inner_not_live_store() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let inner: Arc<dyn ChainStore> = Arc::new(InMemoryChainStore::new());
        let inner_clone = inner.clone();

        // Use an atomic variable to check if the overridden function was called.
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_clone = calls.clone();
        let store = OverridingChainStore::builder(inner)
            .with_load_from_best_chain(move |store, point| {
                calls_clone.fetch_add(1, Ordering::SeqCst);
                store.load_from_best_chain(point)
            })
            .build();

        let snapshot = store.snapshot();
        let chain = create_best_chain(inner_clone.as_ref(), 1);
        let result = snapshot.load_from_best_chain(&chain[0].point());

        assert!(calls.load(Ordering::SeqCst) > 0, "override must be called");
        assert!(result.is_none(), "the snapshot must not return a best chain header");
    }

    // HELPERS

    /// Create a best chain of size `len` and return its headers from older to most recent.
    fn create_best_chain(store: &dyn ChainStore, len: usize) -> Vec<BlockHeader> {
        let mut headers = Vec::with_capacity(len);
        for i in 0..len {
            let parent = headers.last().map(BlockHeader::hash);
            let header = BlockHeader::from(make_header((i + 1) as u64, (i + 1) as u64, parent));
            store.store_header(&header).unwrap();
            store.roll_forward_chain(&header.point()).unwrap();
            headers.push(header);
        }
        headers
    }
}
