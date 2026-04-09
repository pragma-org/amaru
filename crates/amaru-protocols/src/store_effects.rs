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

use std::sync::Arc;

use amaru_kernel::{BlockHeader, GlobalParameters, HeaderHash, Point, RawBlock};
use amaru_ouroboros_traits::{ChainStore, MissingBlockRange, Nonces, StoreError};
use pure_stage::{BoxFuture, Effects, ExternalEffect, ExternalEffectAPI, Resources, SendData, Void};

/// Implementation of ChainStore using pure_stage::Effects.
#[derive(Clone, Debug)]
pub struct Store {
    effects: Effects<Void>,
}

impl Store {
    pub fn new<T: SendData>(effects: Effects<T>) -> Store {
        Store { effects: effects.erase() }
    }

    pub fn external<E: ExternalEffectAPI + 'static>(&self, effect: E) -> BoxFuture<'static, E::Response> {
        self.effects.external(effect)
    }

    pub fn load_header(&self, hash: &HeaderHash) -> BoxFuture<'static, Option<BlockHeader>> {
        self.external(LoadHeaderEffect::new(*hash))
    }

    pub fn load_header_with_validity(
        &self,
        hash: &HeaderHash,
    ) -> BoxFuture<'static, Option<(BlockHeader, Option<bool>)>> {
        self.external(LoadHeaderWithValidityEffect::new(*hash))
    }

    pub fn get_children(&self, hash: &HeaderHash) -> BoxFuture<'static, Vec<HeaderHash>> {
        self.external(GetChildrenEffect::new(*hash))
    }

    pub fn get_anchor_hash(&self) -> BoxFuture<'static, HeaderHash> {
        self.external(GetAnchorHashEffect::new())
    }

    pub fn get_best_chain_hash(&self) -> BoxFuture<'static, HeaderHash> {
        self.external(GetBestChainHashEffect::new())
    }

    pub fn load_block(&self, hash: &HeaderHash) -> BoxFuture<'static, Result<Option<RawBlock>, StoreError>> {
        self.external(LoadBlockEffect::new(*hash))
    }

    pub fn get_nonces(&self, hash: &HeaderHash) -> BoxFuture<'static, Option<Nonces>> {
        self.external(GetNoncesEffect::new(*hash))
    }

    pub fn has_header(&self, hash: &HeaderHash) -> BoxFuture<'static, bool> {
        self.external(HasHeaderEffect::new(*hash))
    }

    pub fn load_from_best_chain(&self, point: &Point) -> BoxFuture<'static, Option<HeaderHash>> {
        self.external(LoadFromBestChainEffect::new(*point))
    }

    pub fn next_best_chain(&self, point: &Point) -> BoxFuture<'static, Option<Point>> {
        self.external(NextBestChainEffect::new(*point))
    }

    pub fn set_block_valid(&self, hash: &HeaderHash, valid: bool) -> BoxFuture<'static, Result<(), StoreError>> {
        self.external(SetBlockValidEffect::new(*hash, valid))
    }

    pub fn set_anchor_hash(&self, hash: &HeaderHash) -> BoxFuture<'static, Result<(), StoreError>> {
        self.external(SetAnchorHashEffect::new(*hash))
    }

    pub fn set_best_chain_hash(&self, hash: &HeaderHash) -> BoxFuture<'static, Result<(), StoreError>> {
        self.external(SetBestChainHashEffect::new(*hash))
    }

    pub fn store_header(&self, header: &BlockHeader) -> BoxFuture<'static, Result<(), StoreError>> {
        self.external(StoreHeaderEffect::new(header.clone()))
    }

    pub fn store_block(&self, hash: &HeaderHash, block: &RawBlock) -> BoxFuture<'static, Result<(), StoreError>> {
        self.external(StoreBlockEffect::new(hash, block.clone()))
    }

    pub fn put_nonces(&self, header: &HeaderHash, nonces: &Nonces) -> BoxFuture<'static, Result<(), StoreError>> {
        self.external(PutNoncesEffect::new(*header, nonces.clone()))
    }

    pub fn roll_forward_chain(&self, point: &Point) -> BoxFuture<'static, Result<(), StoreError>> {
        self.external(RollForwardChainEffect::new(*point))
    }

    pub fn rollback_chain(&self, point: &Point) -> BoxFuture<'static, Result<usize, StoreError>> {
        self.external(RollBackChainEffect::new(*point))
    }

    pub fn load_tip(&self, hash: &HeaderHash) -> BoxFuture<'static, Option<amaru_kernel::Tip>> {
        let store = self.clone();
        let hash = *hash;
        Box::pin(async move {
            if hash == amaru_kernel::ORIGIN_HASH {
                return Some(amaru_kernel::Tip::origin());
            }
            store.load_header(&hash).await.map(|h| h.tip())
        })
    }
}

// ASYNC QUERY METHODS (single-effect operations for simulation compatibility)

impl Store {
    pub fn unvalidated_ancestor_hashes(&self, start: HeaderHash) -> BoxFuture<'static, (Vec<HeaderHash>, bool)> {
        self.effects.external(UnvalidatedAncestorHashesEffect::new(start))
    }

    pub fn find_fork_point(&self, start: HeaderHash) -> BoxFuture<'static, Option<(Point, Vec<Point>)>> {
        self.effects.external(FindForkPointEffect::new(start))
    }

    pub fn find_common_ancestor(&self, hash_a: HeaderHash, hash_b: HeaderHash) -> BoxFuture<'static, Option<Point>> {
        self.effects.external(FindCommonAncestorEffect::new(hash_a, hash_b))
    }

    pub fn find_intersect_point(&self, points: Vec<Point>) -> BoxFuture<'static, Option<Point>> {
        self.effects.external(FindIntersectPointEffect::new(points))
    }

    pub fn sample_ancestor_points(&self) -> BoxFuture<'static, Vec<Point>> {
        self.effects.external(SampleAncestorPointsEffect::new())
    }

    pub fn find_missing_blocks(
        &self,
        start: HeaderHash,
        limit: usize,
    ) -> BoxFuture<'static, Result<Option<MissingBlockRange>, StoreError>> {
        self.effects.external(FindMissingBlocksEffect::new(start, limit))
    }
}

// EXTERNAL EFFECTS DEFINITIONS

pub type ResourceHeaderStore = Arc<dyn ChainStore<BlockHeader>>;
pub type ResourceParameters = GlobalParameters;

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoreHeaderEffect {
    header: BlockHeader,
}

impl StoreHeaderEffect {
    pub fn new(header: BlockHeader) -> Self {
        Self { header }
    }
}

impl ExternalEffect for StoreHeaderEffect {
    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap_sync({
            let store =
                resources.get::<ResourceHeaderStore>().expect("StoreHeaderEffect requires a chain store").clone();
            store.store_header(&self.header)
        })
    }
}

impl ExternalEffectAPI for StoreHeaderEffect {
    type Response = Result<(), StoreError>;
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoreBlockEffect {
    hash: HeaderHash,
    block: RawBlock,
}

impl StoreBlockEffect {
    pub fn new(hash: &HeaderHash, block: RawBlock) -> Self {
        Self { hash: *hash, block }
    }
}

impl ExternalEffect for StoreBlockEffect {
    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap_sync({
            let store =
                resources.get::<ResourceHeaderStore>().expect("StoreBlockEffect requires a chain store").clone();
            store.store_block(&self.hash, &self.block)
        })
    }
}

impl ExternalEffectAPI for StoreBlockEffect {
    type Response = Result<(), StoreError>;
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SetAnchorHashEffect {
    hash: HeaderHash,
}

impl SetAnchorHashEffect {
    pub fn new(hash: HeaderHash) -> Self {
        Self { hash }
    }
}

impl ExternalEffect for SetAnchorHashEffect {
    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap_sync({
            let store =
                resources.get::<ResourceHeaderStore>().expect("SetAnchorHashEffect requires a chain store").clone();
            store.set_anchor_hash(&self.hash)
        })
    }
}

impl ExternalEffectAPI for SetAnchorHashEffect {
    type Response = Result<(), StoreError>;
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SetBestChainHashEffect {
    hash: HeaderHash,
}

impl SetBestChainHashEffect {
    pub fn new(hash: HeaderHash) -> Self {
        Self { hash }
    }
}

impl ExternalEffect for SetBestChainHashEffect {
    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap_sync({
            let store =
                resources.get::<ResourceHeaderStore>().expect("SetBestChainHashEffect requires a chain store").clone();
            store.set_best_chain_hash(&self.hash)
        })
    }
}

impl ExternalEffectAPI for SetBestChainHashEffect {
    type Response = Result<(), StoreError>;
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PutNoncesEffect {
    hash: HeaderHash,
    nonces: Nonces,
}

impl PutNoncesEffect {
    pub fn new(hash: HeaderHash, nonces: Nonces) -> Self {
        Self { hash, nonces }
    }
}

impl ExternalEffect for PutNoncesEffect {
    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap_sync({
            let store = resources.get::<ResourceHeaderStore>().expect("PutNoncesEffect requires a chain store").clone();
            store.put_nonces(&self.hash, &self.nonces)
        })
    }
}

impl ExternalEffectAPI for PutNoncesEffect {
    type Response = Result<(), StoreError>;
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct HasHeaderEffect {
    hash: HeaderHash,
}

impl HasHeaderEffect {
    pub fn new(hash: HeaderHash) -> Self {
        Self { hash }
    }
}

impl ExternalEffect for HasHeaderEffect {
    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap_sync({
            let store = resources.get::<ResourceHeaderStore>().expect("HasHeaderEffect requires a chain store").clone();
            store.has_header(&self.hash)
        })
    }
}

impl ExternalEffectAPI for HasHeaderEffect {
    type Response = bool;
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LoadFromBestChainEffect {
    point: Point,
}

impl LoadFromBestChainEffect {
    pub fn new(point: Point) -> Self {
        Self { point }
    }
}

impl ExternalEffect for LoadFromBestChainEffect {
    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap_sync({
            let store =
                resources.get::<ResourceHeaderStore>().expect("LoadFromBestChainEffect requires a chain store").clone();
            store.load_from_best_chain(&self.point)
        })
    }
}

impl ExternalEffectAPI for LoadFromBestChainEffect {
    type Response = Option<HeaderHash>;
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct NextBestChainEffect {
    point: Point,
}

impl NextBestChainEffect {
    pub fn new(point: Point) -> Self {
        Self { point }
    }
}

impl ExternalEffect for NextBestChainEffect {
    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap_sync({
            let store =
                resources.get::<ResourceHeaderStore>().expect("NextBestChainEffect requires a chain store").clone();
            store.next_best_chain(&self.point)
        })
    }
}

impl ExternalEffectAPI for NextBestChainEffect {
    type Response = Option<Point>;
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LoadHeaderEffect {
    hash: HeaderHash,
}

impl LoadHeaderEffect {
    pub fn new(hash: HeaderHash) -> Self {
        Self { hash }
    }
}

impl ExternalEffect for LoadHeaderEffect {
    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap_sync({
            let store =
                resources.get::<ResourceHeaderStore>().expect("LoadHeaderEffect requires a chain store").clone();
            store.load_header(&self.hash)
        })
    }
}

impl ExternalEffectAPI for LoadHeaderEffect {
    type Response = Option<BlockHeader>;
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LoadHeaderWithValidityEffect {
    hash: HeaderHash,
}

impl LoadHeaderWithValidityEffect {
    pub fn new(hash: HeaderHash) -> Self {
        Self { hash }
    }
}

impl ExternalEffect for LoadHeaderWithValidityEffect {
    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap_sync({
            let store = resources
                .get::<ResourceHeaderStore>()
                .expect("LoadHeaderWithValidityEffect requires a chain store")
                .clone();
            store.load_header_with_validity(&self.hash)
        })
    }
}

impl ExternalEffectAPI for LoadHeaderWithValidityEffect {
    type Response = Option<(BlockHeader, Option<bool>)>;
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SetBlockValidEffect {
    hash: HeaderHash,
    valid: bool,
}

impl SetBlockValidEffect {
    pub fn new(hash: HeaderHash, valid: bool) -> Self {
        Self { hash, valid }
    }
}

impl ExternalEffect for SetBlockValidEffect {
    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap_sync({
            let store =
                resources.get::<ResourceHeaderStore>().expect("SetBlockValidEffect requires a chain store").clone();
            store.set_block_valid(&self.hash, self.valid)
        })
    }
}

impl ExternalEffectAPI for SetBlockValidEffect {
    type Response = Result<(), StoreError>;
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GetChildrenEffect {
    hash: HeaderHash,
}

impl GetChildrenEffect {
    pub fn new(hash: HeaderHash) -> Self {
        Self { hash }
    }
}

impl ExternalEffect for GetChildrenEffect {
    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap_sync({
            let store =
                resources.get::<ResourceHeaderStore>().expect("GetChildrenEffect requires a chain store").clone();
            store.get_children(&self.hash)
        })
    }
}

impl ExternalEffectAPI for GetChildrenEffect {
    type Response = Vec<HeaderHash>;
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GetAnchorHashEffect;

impl GetAnchorHashEffect {
    #[expect(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {}
    }
}

impl ExternalEffect for GetAnchorHashEffect {
    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap_sync({
            let store =
                resources.get::<ResourceHeaderStore>().expect("GetAnchorHashEffect requires a chain store").clone();
            store.get_anchor_hash()
        })
    }
}

impl ExternalEffectAPI for GetAnchorHashEffect {
    type Response = HeaderHash;
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GetBestChainHashEffect;

impl GetBestChainHashEffect {
    #[expect(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {}
    }
}

impl ExternalEffect for GetBestChainHashEffect {
    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap_sync({
            let store =
                resources.get::<ResourceHeaderStore>().expect("GetBestChainHashEffect requires a chain store").clone();
            store.get_best_chain_hash()
        })
    }
}

impl ExternalEffectAPI for GetBestChainHashEffect {
    type Response = HeaderHash;
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LoadBlockEffect {
    hash: HeaderHash,
}

impl LoadBlockEffect {
    pub fn new(hash: HeaderHash) -> Self {
        Self { hash }
    }
}

impl ExternalEffect for LoadBlockEffect {
    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap_sync({
            let store = resources.get::<ResourceHeaderStore>().expect("LoadBlockEffect requires a chain store").clone();
            store.load_block(&self.hash)
        })
    }
}

impl ExternalEffectAPI for LoadBlockEffect {
    type Response = Result<Option<RawBlock>, StoreError>;
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GetNoncesEffect {
    hash: HeaderHash,
}

impl GetNoncesEffect {
    pub fn new(hash: HeaderHash) -> Self {
        Self { hash }
    }
}

impl ExternalEffect for GetNoncesEffect {
    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap_sync({
            let store = resources.get::<ResourceHeaderStore>().expect("GetNoncesEffect requires a chain store").clone();
            store.get_nonces(&self.hash)
        })
    }
}

impl ExternalEffectAPI for GetNoncesEffect {
    type Response = Option<Nonces>;
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RollForwardChainEffect {
    point: Point,
}

impl RollForwardChainEffect {
    pub fn new(point: Point) -> Self {
        Self { point }
    }
}

impl ExternalEffect for RollForwardChainEffect {
    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap_sync({
            let store =
                resources.get::<ResourceHeaderStore>().expect("RollForwardChainEffect requires a chain store").clone();
            store.roll_forward_chain(&self.point)
        })
    }
}

impl ExternalEffectAPI for RollForwardChainEffect {
    type Response = Result<(), StoreError>;
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RollBackChainEffect {
    point: Point,
}

impl RollBackChainEffect {
    pub fn new(point: Point) -> Self {
        Self { point }
    }
}

impl ExternalEffect for RollBackChainEffect {
    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap_sync({
            let store =
                resources.get::<ResourceHeaderStore>().expect("RollBackChainEffect requires a chain store").clone();
            store.rollback_chain(&self.point)
        })
    }
}

impl ExternalEffectAPI for RollBackChainEffect {
    type Response = Result<usize, StoreError>;
}

// TARGETED QUERY EFFECTS (single atomic operations replacing iterator-based patterns)

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct UnvalidatedAncestorHashesEffect {
    start: HeaderHash,
}

impl UnvalidatedAncestorHashesEffect {
    pub fn new(start: HeaderHash) -> Self {
        Self { start }
    }
}

impl ExternalEffect for UnvalidatedAncestorHashesEffect {
    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap_sync({
            let store = resources
                .get::<ResourceHeaderStore>()
                .expect("UnvalidatedAncestorHashesEffect requires a chain store")
                .clone();
            let snapshot = store.snapshot();
            snapshot.unvalidated_ancestor_hashes(self.start)
        })
    }
}

impl ExternalEffectAPI for UnvalidatedAncestorHashesEffect {
    type Response = (Vec<HeaderHash>, bool);
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FindForkPointEffect {
    start: HeaderHash,
}

impl FindForkPointEffect {
    pub fn new(start: HeaderHash) -> Self {
        Self { start }
    }
}

impl ExternalEffect for FindForkPointEffect {
    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap_sync({
            let store =
                resources.get::<ResourceHeaderStore>().expect("FindForkPointEffect requires a chain store").clone();
            let snapshot = store.snapshot();
            snapshot.find_fork_point(self.start)
        })
    }
}

impl ExternalEffectAPI for FindForkPointEffect {
    type Response = Option<(Point, Vec<Point>)>;
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FindCommonAncestorEffect {
    hash_a: HeaderHash,
    hash_b: HeaderHash,
}

impl FindCommonAncestorEffect {
    pub fn new(hash_a: HeaderHash, hash_b: HeaderHash) -> Self {
        Self { hash_a, hash_b }
    }
}

impl ExternalEffect for FindCommonAncestorEffect {
    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap_sync({
            let store = resources
                .get::<ResourceHeaderStore>()
                .expect("FindCommonAncestorEffect requires a chain store")
                .clone();
            let snapshot = store.snapshot();
            snapshot.find_common_ancestor(self.hash_a, self.hash_b)
        })
    }
}

impl ExternalEffectAPI for FindCommonAncestorEffect {
    type Response = Option<Point>;
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FindIntersectPointEffect {
    points: Vec<Point>,
}

impl FindIntersectPointEffect {
    pub fn new(points: Vec<Point>) -> Self {
        Self { points }
    }
}

impl ExternalEffect for FindIntersectPointEffect {
    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap_sync({
            let store = resources
                .get::<ResourceHeaderStore>()
                .expect("FindIntersectPointEffect requires a chain store")
                .clone();
            store.find_intersect_point(self.points)
        })
    }
}

impl ExternalEffectAPI for FindIntersectPointEffect {
    type Response = Option<Point>;
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SampleAncestorPointsEffect;

impl SampleAncestorPointsEffect {
    #[expect(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {}
    }
}

impl ExternalEffect for SampleAncestorPointsEffect {
    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap_sync({
            let store = resources
                .get::<ResourceHeaderStore>()
                .expect("SampleAncestorPointsEffect requires a chain store")
                .clone();
            let snapshot = store.snapshot();
            snapshot.sample_ancestor_points()
        })
    }
}

impl ExternalEffectAPI for SampleAncestorPointsEffect {
    type Response = Vec<Point>;
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FindMissingBlocksEffect {
    start: HeaderHash,
    limit: usize,
}

impl FindMissingBlocksEffect {
    pub fn new(start: HeaderHash, limit: usize) -> Self {
        Self { start, limit }
    }
}

impl ExternalEffect for FindMissingBlocksEffect {
    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap_sync({
            let store =
                resources.get::<ResourceHeaderStore>().expect("FindMissingBlocksEffect requires a chain store").clone();
            let snapshot = store.snapshot();
            snapshot.find_missing_blocks(self.start, self.limit)
        })
    }
}

impl ExternalEffectAPI for FindMissingBlocksEffect {
    type Response = Result<Option<MissingBlockRange>, StoreError>;
}
