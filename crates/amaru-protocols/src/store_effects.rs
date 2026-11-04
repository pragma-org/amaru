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

use amaru_kernel::{BlockHeight, GlobalParameters, Header, HeaderHash, NetworkPoint, NonEmptyVec, Point, RawBlock};
use amaru_observability::{AttachedSpan, NoParent, TraceContext};
use amaru_ouroboros_traits::{
    ChainStore, FindAncestorOnBestChainResult, FindCommonAncestorResult, MissingBlocksResult, NextBestChainHeader,
    Nonces, ReadChainStore, SampleAncestorPointsResult, StoreError,
};
use amaru_pure_stage::{
    BoxFuture, DeserializerGuards, DurationDist, Effects, ExternalEffectAPI, Resources, SendData, Void,
};

use crate::effect_timings;

/// Factory for chain-store external effects.
///
/// Stages can construct an effect and pass it to `eff.external(...)`:
///
/// ```ignore
/// let point = eff.external(StoreEffect::load_point(hash)).await;
/// ```
///
/// [`Store`] remains available for existing call sites.
pub struct StoreEffect;

impl StoreEffect {
    pub fn load_header(hash: HeaderHash) -> LoadHeaderEffect {
        LoadHeaderEffect::new(hash)
    }

    pub fn load_header_with_validity(hash: HeaderHash) -> LoadHeaderWithValidityEffect {
        LoadHeaderWithValidityEffect::new(hash)
    }

    pub fn get_children(hash: HeaderHash) -> GetChildrenEffect {
        GetChildrenEffect::new(hash)
    }

    pub fn get_anchor_hash() -> GetAnchorHashEffect {
        GetAnchorHashEffect::new()
    }

    pub fn get_best_chain_hash() -> GetBestChainHashEffect {
        GetBestChainHashEffect::new()
    }

    pub fn get_best_chain_tip() -> GetBestChainTipEffect {
        GetBestChainTipEffect::new()
    }

    pub fn load_block(hash: HeaderHash) -> LoadBlockEffect {
        LoadBlockEffect::new(hash)
    }

    pub fn has_block(hash: HeaderHash) -> HasBlockEffect {
        HasBlockEffect::new(hash)
    }

    pub fn get_nonces(hash: HeaderHash) -> GetNoncesEffect {
        GetNoncesEffect::new(hash)
    }

    pub fn has_header(hash: HeaderHash) -> HasHeaderEffect {
        HasHeaderEffect::new(hash)
    }

    pub fn is_on_best_chain(point: impl Into<NetworkPoint>) -> IsOnBestChainEffect {
        IsOnBestChainEffect::new(point.into())
    }

    pub fn next_best_chain(point: Point) -> NextBestChainEffect {
        NextBestChainEffect::new(point)
    }

    pub fn next_best_chain_header(point: Point) -> NextBestChainHeaderEffect {
        NextBestChainHeaderEffect::new(point)
    }

    pub fn set_block_valid(hash: HeaderHash, valid: bool) -> SetBlockValidEffect {
        SetBlockValidEffect::new(hash, valid)
    }

    pub fn set_anchor_point(point: Point) -> SetAnchorPointEffect {
        SetAnchorPointEffect::new(point)
    }

    pub fn set_best_chain_tip(tip: Point) -> SetBestChainTipEffect {
        SetBestChainTipEffect::new(tip)
    }

    pub fn store_validated_header(header: Header, nonces: Nonces) -> StoreValidatedHeaderEffect {
        StoreValidatedHeaderEffect::new(header, nonces)
    }

    pub fn store_block(hash: HeaderHash, block: RawBlock) -> StoreBlockEffect {
        StoreBlockEffect::new(&hash, block)
    }

    pub fn put_nonces(header: HeaderHash, nonces: Nonces) -> PutNoncesEffect {
        PutNoncesEffect::new(header, nonces)
    }

    pub fn switch_to_fork(fork_point: Point, forward_points: NonEmptyVec<Point>) -> SwitchToForkEffect {
        SwitchToForkEffect::new(fork_point, forward_points)
    }

    pub fn roll_forward_chain(point: Point) -> RollForwardChainEffect {
        RollForwardChainEffect::new(point)
    }

    pub fn load_point(hash: HeaderHash) -> LoadPointEffect {
        LoadPointEffect::new(hash)
    }

    pub fn unvalidated_ancestor_hashes(start: HeaderHash) -> UnvalidatedAncestorHashesEffect {
        UnvalidatedAncestorHashesEffect::new(start)
    }

    pub fn ancestors_between(from: Point, to: HeaderHash) -> AncestorsBetweenEffect {
        AncestorsBetweenEffect::new(from, to)
    }

    pub fn find_ancestor_on_best_chain(start: HeaderHash) -> FindAncestorOnBestChainEffect {
        FindAncestorOnBestChainEffect::new(start)
    }

    pub fn find_common_ancestor(hash_a: HeaderHash, hash_b: HeaderHash) -> FindCommonAncestorEffect {
        FindCommonAncestorEffect::new(hash_a, hash_b)
    }

    pub fn find_intersect_point(points: Vec<NetworkPoint>) -> FindIntersectPointEffect {
        FindIntersectPointEffect::new(points)
    }

    pub fn sample_ancestor_points() -> SampleAncestorPointsEffect {
        SampleAncestorPointsEffect::new()
    }

    pub fn find_anchor_at_height(target_height: BlockHeight) -> FindAnchorAtHeightEffect {
        FindAnchorAtHeightEffect::new(target_height)
    }

    pub fn find_missing_blocks(start: HeaderHash, limit: usize) -> FindMissingBlocksEffect {
        FindMissingBlocksEffect::new(start, limit)
    }

    pub fn get_block_points_range(
        from: NetworkPoint,
        to: NetworkPoint,
        request: RangeRequest,
    ) -> GetBlockPointsRangeEffect {
        GetBlockPointsRangeEffect::new(from, to, request)
    }
}

/// Implementation of ChainStore using amaru_pure_stage::Effects.
///
/// `S` is the span the context was taken from. Store reads that do not open a
/// child span keep those bytes and do not treat `S` as a parent list.
#[derive(Clone, Debug)]
pub struct Store<S = NoParent> {
    effects: Effects<Void>,
    trace_context: TraceContext<S>,
}

impl Store<NoParent> {
    pub fn new<T: SendData>(effects: Effects<T>) -> Self {
        Store { effects: effects.erase(), trace_context: TraceContext::none() }
    }
}

impl<S> Store<S> {
    pub fn with_trace_context<P>(self, trace_context: &TraceContext<P>) -> Store<P> {
        Store { effects: self.effects, trace_context: trace_context.clone() }
    }

    pub fn load_header(&self, hash: &HeaderHash) -> BoxFuture<'static, Option<Header>> {
        self.effects.external(StoreEffect::load_header(*hash))
    }

    pub fn load_header_with_validity(&self, hash: &HeaderHash) -> BoxFuture<'static, Option<(Header, Option<bool>)>> {
        self.effects.external(StoreEffect::load_header_with_validity(*hash))
    }

    pub fn get_children(&self, hash: &HeaderHash) -> BoxFuture<'static, Vec<HeaderHash>> {
        self.effects.external(StoreEffect::get_children(*hash))
    }

    pub fn get_anchor_hash(&self) -> BoxFuture<'static, HeaderHash> {
        self.effects.external(StoreEffect::get_anchor_hash())
    }

    pub fn get_best_chain_hash(&self) -> BoxFuture<'static, HeaderHash> {
        self.effects.external(StoreEffect::get_best_chain_hash())
    }

    pub fn get_best_chain_tip(&self) -> BoxFuture<'static, Point> {
        self.effects.external(StoreEffect::get_best_chain_tip())
    }

    pub fn load_block(&self, hash: &HeaderHash) -> BoxFuture<'static, Result<Option<RawBlock>, StoreError>> {
        self.effects.external(StoreEffect::load_block(*hash))
    }

    pub fn has_block(&self, hash: &HeaderHash) -> BoxFuture<'static, Result<bool, StoreError>> {
        self.effects.external(StoreEffect::has_block(*hash))
    }

    pub fn get_nonces(&self, hash: &HeaderHash) -> BoxFuture<'static, Option<Nonces>> {
        self.effects.external(StoreEffect::get_nonces(*hash))
    }

    pub fn has_header(&self, hash: &HeaderHash) -> BoxFuture<'static, bool> {
        self.effects.external(StoreEffect::has_header(*hash))
    }

    pub fn is_on_best_chain(&self, point: impl Into<NetworkPoint>) -> BoxFuture<'static, bool> {
        self.effects.external(StoreEffect::is_on_best_chain(point))
    }

    pub fn next_best_chain(&self, point: &Point) -> BoxFuture<'static, Option<Point>> {
        self.effects.external(StoreEffect::next_best_chain(*point))
    }

    pub fn next_best_chain_header(&self, point: &Point) -> BoxFuture<'static, Result<NextBestChainHeader, StoreError>> {
        self.effects.external(StoreEffect::next_best_chain_header(*point))
    }

    pub fn set_block_valid(&self, hash: &HeaderHash, valid: bool) -> BoxFuture<'static, Result<(), StoreError>> {
        self.effects.external(StoreEffect::set_block_valid(*hash, valid))
    }

    pub fn set_anchor_point(&self, point: &Point) -> BoxFuture<'static, Result<(), StoreError>> {
        self.effects.external(StoreEffect::set_anchor_point(*point))
    }

    pub fn set_best_chain_tip(&self, tip: &Point) -> BoxFuture<'static, Result<(), StoreError>> {
        self.effects.external(StoreEffect::set_best_chain_tip(*tip))
    }

    pub fn store_validated_header(
        &self,
        header: &Header,
        nonces: &Nonces,
    ) -> BoxFuture<'static, Result<(), StoreError>> {
        self.effects.external(StoreEffect::store_validated_header(header.clone(), nonces.clone()))
    }

    pub fn store_block(&self, hash: &HeaderHash, block: &RawBlock) -> BoxFuture<'static, Result<(), StoreError>> {
        self.effects.external(StoreEffect::store_block(*hash, block.clone()))
    }

    pub fn put_nonces(&self, header: &HeaderHash, nonces: &Nonces) -> BoxFuture<'static, Result<(), StoreError>> {
        self.effects.external(StoreEffect::put_nonces(*header, nonces.clone()))
    }

    pub fn switch_to_fork(
        &self,
        fork_point: &Point,
        forward_points: &NonEmptyVec<Point>,
    ) -> BoxFuture<'static, Result<(), StoreError>> {
        self.effects.external(StoreEffect::switch_to_fork(*fork_point, forward_points.clone()))
    }

    pub fn roll_forward_chain(&self, point: &Point) -> BoxFuture<'static, Result<(), StoreError>> {
        self.effects.external(StoreEffect::roll_forward_chain(*point))
    }

    pub fn load_point(&self, hash: &HeaderHash) -> BoxFuture<'static, Option<Point>> {
        self.effects.external(StoreEffect::load_point(*hash).with_trace_context(&self.trace_context.for_attach()))
    }

    pub fn unvalidated_ancestor_hashes(&self, start: HeaderHash) -> BoxFuture<'static, (Vec<HeaderHash>, bool)> {
        self.effects.external(StoreEffect::unvalidated_ancestor_hashes(start))
    }

    pub fn ancestors_between(&self, from: Point, to: HeaderHash) -> BoxFuture<'static, Option<Vec<Point>>> {
        self.effects
            .external(StoreEffect::ancestors_between(from, to).with_trace_context(&self.trace_context.for_attach()))
    }

    pub fn find_ancestor_on_best_chain(
        &self,
        start: HeaderHash,
    ) -> BoxFuture<'static, Result<FindAncestorOnBestChainResult, StoreError>> {
        self.effects.external(StoreEffect::find_ancestor_on_best_chain(start))
    }

    pub fn find_common_ancestor(
        &self,
        hash_a: HeaderHash,
        hash_b: HeaderHash,
    ) -> BoxFuture<'static, Result<FindCommonAncestorResult, StoreError>> {
        self.effects.external(StoreEffect::find_common_ancestor(hash_a, hash_b))
    }

    pub fn find_intersect_point(&self, points: Vec<NetworkPoint>) -> BoxFuture<'static, Option<Point>> {
        self.effects.external(StoreEffect::find_intersect_point(points))
    }

    pub fn sample_ancestor_points(&self) -> BoxFuture<'static, Result<SampleAncestorPointsResult, StoreError>> {
        self.effects.external(StoreEffect::sample_ancestor_points())
    }

    pub fn find_anchor_at_height(&self, target_height: BlockHeight) -> BoxFuture<'static, Option<Point>> {
        self.effects.external(StoreEffect::find_anchor_at_height(target_height))
    }

    pub fn find_missing_blocks(
        &self,
        start: HeaderHash,
        limit: usize,
    ) -> BoxFuture<'static, Result<MissingBlocksResult, StoreError>> {
        self.effects.external(StoreEffect::find_missing_blocks(start, limit))
    }

    /// Return one page of the block points between `from` and `to` (see [`BlockPointsPage`]).
    pub fn get_block_points_range(
        &self,
        from: NetworkPoint,
        to: NetworkPoint,
        request: RangeRequest,
    ) -> BoxFuture<'static, BlockPointsPage> {
        self.effects.external(StoreEffect::get_block_points_range(from, to, request))
    }
}

// EXTERNAL EFFECTS DEFINITIONS

pub type ResourceHeaderStore = Arc<dyn ChainStore>;
pub type ResourceParameters = GlobalParameters;

pub fn register_deserializers() -> DeserializerGuards {
    vec![
        amaru_pure_stage::register_effect_deserializer::<StoreValidatedHeaderEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<StoreBlockEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<SetAnchorPointEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<SetBestChainTipEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<PutNoncesEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<HasHeaderEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<IsOnBestChainEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<NextBestChainEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<NextBestChainHeaderEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<LoadHeaderEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<LoadPointEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<LoadHeaderWithValidityEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<SetBlockValidEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<GetChildrenEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<GetAnchorHashEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<GetBestChainHashEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<GetBestChainTipEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<LoadBlockEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<HasBlockEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<GetNoncesEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<SwitchToForkEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<RollForwardChainEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<UnvalidatedAncestorHashesEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<AncestorsBetweenEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<FindAncestorOnBestChainEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<FindAnchorAtHeightEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<FindCommonAncestorEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<FindIntersectPointEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<SampleAncestorPointsEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<FindMissingBlocksEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<GetBlockPointsRangeEffect>().boxed(),
    ]
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoreValidatedHeaderEffect {
    header: Header,
    nonces: Nonces,
}

impl StoreValidatedHeaderEffect {
    pub fn new(header: Header, nonces: Nonces) -> Self {
        Self { header, nonces }
    }
}

impl ExternalEffectAPI for StoreValidatedHeaderEffect {
    type Response = Result<(), StoreError>;
    const SIMULATED_DURATION: DurationDist = effect_timings::store_effects__StoreValidatedHeaderEffect;

    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let store = resources
                .get::<ResourceHeaderStore>()
                .expect("StoreValidatedHeaderEffect requires a chain store")
                .clone();
            store.store_validated_header(&self.header, &self.nonces)
        })
    }
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

impl ExternalEffectAPI for StoreBlockEffect {
    type Response = Result<(), StoreError>;
    const SIMULATED_DURATION: DurationDist = effect_timings::store_effects__StoreBlockEffect;

    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let store =
                resources.get::<ResourceHeaderStore>().expect("StoreBlockEffect requires a chain store").clone();
            store.store_block(&self.hash, &self.block)
        })
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SetAnchorPointEffect {
    point: Point,
}

impl SetAnchorPointEffect {
    pub fn new(point: Point) -> Self {
        Self { point }
    }
}

impl ExternalEffectAPI for SetAnchorPointEffect {
    type Response = Result<(), StoreError>;
    const SIMULATED_DURATION: DurationDist = effect_timings::store_effects__SetAnchorPointEffect;

    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let store =
                resources.get::<ResourceHeaderStore>().expect("SetAnchorPointEffect requires a chain store").clone();
            store.set_anchor_point(&self.point)
        })
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SetBestChainTipEffect {
    tip: Point,
}

impl SetBestChainTipEffect {
    pub fn new(tip: Point) -> Self {
        Self { tip }
    }
}

impl ExternalEffectAPI for SetBestChainTipEffect {
    type Response = Result<(), StoreError>;
    // const SIMULATED_DURATION: DurationDist = effect_timings::store_effects__SetBestChainTipEffect;

    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let store =
                resources.get::<ResourceHeaderStore>().expect("SetBestChainTipEffect requires a chain store").clone();
            store.set_best_chain_tip(&self.tip)
        })
    }
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

impl ExternalEffectAPI for PutNoncesEffect {
    type Response = Result<(), StoreError>;
    // const SIMULATED_DURATION: DurationDist = effect_timings::store_effects__PutNoncesEffect;

    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let store = resources.get::<ResourceHeaderStore>().expect("PutNoncesEffect requires a chain store").clone();
            store.put_nonces(&self.hash, &self.nonces)
        })
    }
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

impl ExternalEffectAPI for HasHeaderEffect {
    type Response = bool;
    const SIMULATED_DURATION: DurationDist = effect_timings::store_effects__HasHeaderEffect;

    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let store = resources.get::<ResourceHeaderStore>().expect("HasHeaderEffect requires a chain store").clone();
            store.has_header(&self.hash)
        })
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct IsOnBestChainEffect {
    point: NetworkPoint,
}

impl IsOnBestChainEffect {
    pub fn new(point: NetworkPoint) -> Self {
        Self { point }
    }
}

impl ExternalEffectAPI for IsOnBestChainEffect {
    type Response = bool;
    // const SIMULATED_DURATION: DurationDist = effect_timings::store_effects__IsOnBestChainEffect;

    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let store =
                resources.get::<ResourceHeaderStore>().expect("IsOnBestChainEffect requires a chain store").clone();
            store.is_on_best_chain(self.point)
        })
    }
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

impl ExternalEffectAPI for NextBestChainEffect {
    type Response = Option<Point>;
    // const SIMULATED_DURATION: DurationDist = effect_timings::store_effects__NextBestChainEffect;

    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let store =
                resources.get::<ResourceHeaderStore>().expect("NextBestChainEffect requires a chain store").clone();
            store.next_best_chain(&self.point)
        })
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct NextBestChainHeaderEffect {
    point: Point,
}

impl NextBestChainHeaderEffect {
    pub fn new(point: Point) -> Self {
        Self { point }
    }
}

impl ExternalEffectAPI for NextBestChainHeaderEffect {
    type Response = Result<NextBestChainHeader, StoreError>;
    // const SIMULATED_DURATION: DurationDist = effect_timings::store_effects__NextBestChainHeaderEffect;

    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let store = resources
                .get::<ResourceHeaderStore>()
                .expect("NextBestChainHeaderEffect requires a chain store")
                .clone();
            store.next_best_chain_header(&self.point)
        })
    }
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

impl ExternalEffectAPI for LoadHeaderEffect {
    type Response = Option<Header>;
    const SIMULATED_DURATION: DurationDist = effect_timings::store_effects__LoadHeaderEffect;

    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let store =
                resources.get::<ResourceHeaderStore>().expect("LoadHeaderEffect requires a chain store").clone();
            store.load_header(&self.hash)
        })
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LoadPointEffect {
    hash: HeaderHash,
    trace_context: TraceContext<AttachedSpan>,
}

impl LoadPointEffect {
    pub fn new(hash: HeaderHash) -> Self {
        Self { hash, trace_context: TraceContext::detached() }
    }

    /// Accepts the caller's span. This effect only attaches that context.
    pub fn with_trace_context(mut self, trace_context: &TraceContext<AttachedSpan>) -> Self {
        self.trace_context = trace_context.clone();
        self
    }
}

impl ExternalEffectAPI for LoadPointEffect {
    type Response = Option<Point>;
    const SIMULATED_DURATION: DurationDist = effect_timings::store_effects__LoadPointEffect;

    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let _guard = self.trace_context.attach();
            let store = resources.get::<ResourceHeaderStore>().expect("LoadPointEffect requires a chain store").clone();
            store.load_point(&self.hash)
        })
    }
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

impl ExternalEffectAPI for LoadHeaderWithValidityEffect {
    type Response = Option<(Header, Option<bool>)>;
    const SIMULATED_DURATION: DurationDist = effect_timings::store_effects__LoadHeaderWithValidityEffect;

    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let store = resources
                .get::<ResourceHeaderStore>()
                .expect("LoadHeaderWithValidityEffect requires a chain store")
                .clone();
            store.load_header_with_validity(&self.hash)
        })
    }
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

impl ExternalEffectAPI for SetBlockValidEffect {
    type Response = Result<(), StoreError>;
    const SIMULATED_DURATION: DurationDist = effect_timings::store_effects__SetBlockValidEffect;

    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let store =
                resources.get::<ResourceHeaderStore>().expect("SetBlockValidEffect requires a chain store").clone();
            store.set_block_valid(&self.hash, self.valid)
        })
    }
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

impl ExternalEffectAPI for GetChildrenEffect {
    type Response = Vec<HeaderHash>;
    // const SIMULATED_DURATION: DurationDist = effect_timings::store_effects__GetChildrenEffect;

    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let store =
                resources.get::<ResourceHeaderStore>().expect("GetChildrenEffect requires a chain store").clone();
            store.get_children(&self.hash)
        })
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GetAnchorHashEffect;

impl GetAnchorHashEffect {
    #[expect(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {}
    }
}

impl ExternalEffectAPI for GetAnchorHashEffect {
    type Response = HeaderHash;
    // const SIMULATED_DURATION: DurationDist = effect_timings::store_effects__GetAnchorHashEffect;

    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let store =
                resources.get::<ResourceHeaderStore>().expect("GetAnchorHashEffect requires a chain store").clone();
            store.get_anchor_hash()
        })
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GetBestChainHashEffect;

impl GetBestChainHashEffect {
    #[expect(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {}
    }
}

impl ExternalEffectAPI for GetBestChainHashEffect {
    type Response = HeaderHash;
    // const SIMULATED_DURATION: DurationDist = effect_timings::store_effects__GetBestChainHashEffect;

    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let store =
                resources.get::<ResourceHeaderStore>().expect("GetBestChainHashEffect requires a chain store").clone();
            store.get_best_chain_hash()
        })
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GetBestChainTipEffect;

impl GetBestChainTipEffect {
    #[expect(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {}
    }
}

impl ExternalEffectAPI for GetBestChainTipEffect {
    type Response = Point;
    const SIMULATED_DURATION: DurationDist = effect_timings::store_effects__GetBestChainTipEffect;

    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let store =
                resources.get::<ResourceHeaderStore>().expect("GetBestChainTipEffect requires a chain store").clone();
            store.get_best_chain_tip()
        })
    }
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

impl ExternalEffectAPI for LoadBlockEffect {
    type Response = Result<Option<RawBlock>, StoreError>;
    // const SIMULATED_DURATION: DurationDist = effect_timings::store_effects__LoadBlockEffect;

    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let store = resources.get::<ResourceHeaderStore>().expect("LoadBlockEffect requires a chain store").clone();
            store.load_block(&self.hash)
        })
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct HasBlockEffect {
    hash: HeaderHash,
}

impl HasBlockEffect {
    pub fn new(hash: HeaderHash) -> Self {
        Self { hash }
    }
}

impl ExternalEffectAPI for HasBlockEffect {
    type Response = Result<bool, StoreError>;
    const SIMULATED_DURATION: DurationDist = effect_timings::store_effects__HasBlockEffect;

    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let store = resources.get::<ResourceHeaderStore>().expect("HasBlockEffect requires a chain store").clone();
            store.has_block(&self.hash)
        })
    }
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

impl ExternalEffectAPI for GetNoncesEffect {
    type Response = Option<Nonces>;
    const SIMULATED_DURATION: DurationDist = effect_timings::store_effects__GetNoncesEffect;

    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let store = resources.get::<ResourceHeaderStore>().expect("GetNoncesEffect requires a chain store").clone();
            store.get_nonces(&self.hash)
        })
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SwitchToForkEffect {
    fork_point: Point,
    forward_points: NonEmptyVec<Point>,
}

impl SwitchToForkEffect {
    pub fn new(fork_point: Point, forward_points: NonEmptyVec<Point>) -> Self {
        Self { fork_point, forward_points }
    }
}

impl ExternalEffectAPI for SwitchToForkEffect {
    type Response = Result<(), StoreError>;
    const SIMULATED_DURATION: DurationDist = effect_timings::store_effects__SwitchToForkEffect;

    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let store =
                resources.get::<ResourceHeaderStore>().expect("SwitchToForkEffect requires a chain store").clone();
            store.switch_to_fork(&self.fork_point, &self.forward_points)
        })
    }
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

impl ExternalEffectAPI for RollForwardChainEffect {
    type Response = Result<(), StoreError>;
    const SIMULATED_DURATION: DurationDist = effect_timings::store_effects__RollForwardChainEffect;

    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let store =
                resources.get::<ResourceHeaderStore>().expect("RollForwardChainEffect requires a chain store").clone();
            store.roll_forward_chain(&self.point)
        })
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct UnvalidatedAncestorHashesEffect {
    start: HeaderHash,
}

impl UnvalidatedAncestorHashesEffect {
    pub fn new(start: HeaderHash) -> Self {
        Self { start }
    }
}

impl ExternalEffectAPI for UnvalidatedAncestorHashesEffect {
    type Response = (Vec<HeaderHash>, bool);
    const SIMULATED_DURATION: DurationDist = effect_timings::store_effects__UnvalidatedAncestorHashesEffect;

    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let store = resources
                .get::<ResourceHeaderStore>()
                .expect("UnvalidatedAncestorHashesEffect requires a chain store")
                .clone();
            store.unvalidated_ancestor_hashes(self.start)
        })
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AncestorsBetweenEffect {
    from: Point,
    to: HeaderHash,
    trace_context: TraceContext<AttachedSpan>,
}

impl AncestorsBetweenEffect {
    pub fn new(from: Point, to: HeaderHash) -> Self {
        Self { from, to, trace_context: TraceContext::detached() }
    }

    /// Accepts the caller's span. This effect only attaches that context.
    pub fn with_trace_context(mut self, trace_context: &TraceContext<AttachedSpan>) -> Self {
        self.trace_context = trace_context.clone();
        self
    }
}

impl ExternalEffectAPI for AncestorsBetweenEffect {
    type Response = Option<Vec<Point>>;
    const SIMULATED_DURATION: DurationDist = effect_timings::store_effects__AncestorsBetweenEffect;

    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let _guard = self.trace_context.attach();
            let store =
                resources.get::<ResourceHeaderStore>().expect("AncestorsBetweenEffect requires a chain store").clone();
            store.ancestors_between(&self.from, self.to)
        })
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FindAncestorOnBestChainEffect {
    start: HeaderHash,
}

impl FindAncestorOnBestChainEffect {
    pub fn new(start: HeaderHash) -> Self {
        Self { start }
    }
}

impl ExternalEffectAPI for FindAncestorOnBestChainEffect {
    type Response = Result<FindAncestorOnBestChainResult, StoreError>;
    const SIMULATED_DURATION: DurationDist = effect_timings::store_effects__FindAncestorOnBestChainEffect;

    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let store = resources
                .get::<ResourceHeaderStore>()
                .expect("FindAncestorOnBestChainEffect requires a chain store")
                .clone();
            store.find_ancestor_on_best_chain(self.start)
        })
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FindAnchorAtHeightEffect {
    target_height: BlockHeight,
}

impl FindAnchorAtHeightEffect {
    pub fn new(target_height: BlockHeight) -> Self {
        Self { target_height }
    }
}

impl ExternalEffectAPI for FindAnchorAtHeightEffect {
    type Response = Option<Point>;
    const SIMULATED_DURATION: DurationDist = effect_timings::store_effects__FindAnchorAtHeightEffect;

    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let store = resources
                .get::<ResourceHeaderStore>()
                .expect("FindAnchorAtHeightEffect requires a chain store")
                .clone();
            store.find_anchor_at_height(self.target_height)
        })
    }
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

impl ExternalEffectAPI for FindCommonAncestorEffect {
    type Response = Result<FindCommonAncestorResult, StoreError>;
    // const SIMULATED_DURATION: DurationDist = effect_timings::store_effects__FindCommonAncestorEffect;

    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let store = resources
                .get::<ResourceHeaderStore>()
                .expect("FindCommonAncestorEffect requires a chain store")
                .clone();
            store.find_common_ancestor(self.hash_a, self.hash_b)
        })
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FindIntersectPointEffect {
    points: Vec<NetworkPoint>,
}

impl FindIntersectPointEffect {
    pub fn new(points: Vec<NetworkPoint>) -> Self {
        Self { points }
    }
}

impl ExternalEffectAPI for FindIntersectPointEffect {
    type Response = Option<Point>;
    // const SIMULATED_DURATION: DurationDist = effect_timings::store_effects__FindIntersectPointEffect;

    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let store = resources
                .get::<ResourceHeaderStore>()
                .expect("FindIntersectPointEffect requires a chain store")
                .clone();
            store.find_intersect_point(self.points.clone())
        })
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SampleAncestorPointsEffect;

impl SampleAncestorPointsEffect {
    #[expect(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {}
    }
}

impl ExternalEffectAPI for SampleAncestorPointsEffect {
    type Response = Result<SampleAncestorPointsResult, StoreError>;
    const SIMULATED_DURATION: DurationDist = effect_timings::store_effects__SampleAncestorPointsEffect;

    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let store = resources
                .get::<ResourceHeaderStore>()
                .expect("SampleAncestorPointsEffect requires a chain store")
                .clone();
            store.sample_ancestor_points()
        })
    }
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

impl ExternalEffectAPI for FindMissingBlocksEffect {
    type Response = Result<MissingBlocksResult, StoreError>;
    const SIMULATED_DURATION: DurationDist = effect_timings::store_effects__FindMissingBlocksEffect;

    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let store =
                resources.get::<ResourceHeaderStore>().expect("FindMissingBlocksEffect requires a chain store").clone();
            store.find_missing_blocks(self.start, self.limit)
        })
    }
}

/// A page of the block points of a range: in parent -> child order.
///
/// `next` is the first point that did not fit in the page, if any. Requesting the range again from
/// `next` returns the following page, so a long range is read in constant memory.
///
/// `anchor` is the anchor that the range was read with. The requests for the following pages must use the same
/// anchor, see [`RangeRequest::Next`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BlockPointsPage {
    pub points: Vec<Point>,
    pub next: Option<Point>,
    pub anchor: Point,
}

impl BlockPointsPage {
    pub fn empty() -> Self {
        Self { points: vec![], next: None, anchor: Point::Origin }
    }
}

/// How to read one page of a range, and the maximum number of points in the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RangeRequest {
    /// Read the first page of a range, and check the whole range from `from` to `to`: every block after the anchor
    /// must be in the store and the points must not follow a fork that starts before the anchor.
    /// The first page gives the anchor that was used, and the same anchor must be used for
    /// subsequent requests with `Next`.
    First(usize),

    /// Read the next page of a range that was already checked by `First`, with the anchor that
    /// `First` returned.
    Next { page_size: usize, anchor: Point },
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GetBlockPointsRangeEffect {
    from: NetworkPoint,
    to: NetworkPoint,
    request: RangeRequest,
}

impl GetBlockPointsRangeEffect {
    pub fn new(from: NetworkPoint, to: NetworkPoint, request: RangeRequest) -> Self {
        Self { from, to, request }
    }
}

impl ExternalEffectAPI for GetBlockPointsRangeEffect {
    type Response = BlockPointsPage;

    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let store = resources
                .get::<ResourceHeaderStore>()
                .expect("GetBlockPointsRangeEffect requires a chain store")
                .clone();
            // The range bounds are only known by slot and hash: a bound missing from the store gives an empty page.
            match (store.load_point(&self.from.hash()), store.load_point(&self.to.hash())) {
                (Some(from), Some(to)) => block_points_page(&store, &from, &to, self.request),
                _ => BlockPointsPage::empty(),
            }
        })
    }
}

/// Read one page of the range from `from` to `to`, both inclusive.
fn block_points_page(
    store: &(impl ReadChainStore + ?Sized),
    from: &Point,
    to: &Point,
    request: RangeRequest,
) -> BlockPointsPage {
    match request {
        RangeRequest::First(page_size) => {
            let (anchor, points) = store.get_block_points_in_range_and_anchor(from, to);
            collect_page(points, page_size, anchor)
        }
        RangeRequest::Next { page_size, anchor } => {
            best_chain_page(store, from, to, page_size, anchor).unwrap_or_else(|| {
                collect_page(store.get_block_points_in_range_with_anchor(from, to, &anchor), page_size, anchor)
            })
        }
    }
}

/// Return a full page of `page_size` points up to the anchor, if there is one.
///
/// The range is stopped at the anchor, so the part of the chain after the anchor is not walked. That part is
/// needed only by the last pages of a range.
fn best_chain_page(
    store: &(impl ReadChainStore + ?Sized),
    from: &Point,
    to: &Point,
    page_size: usize,
    anchor: Point,
) -> Option<BlockPointsPage> {
    if *from > anchor || *to <= anchor {
        return None;
    }
    Some(collect_page(store.get_block_points_in_range_with_anchor(from, &anchor, &anchor), page_size, anchor))
        .filter(|page| page.next.is_some())
}

fn collect_page(points: impl Iterator<Item = Point>, page_size: usize, anchor: Point) -> BlockPointsPage {
    // one more point than the page size tells whether there is a next page
    let mut points: Vec<Point> = points.take(page_size.saturating_add(1)).collect();
    let next = if points.len() > page_size { points.pop() } else { None };
    BlockPointsPage { points, next, anchor }
}

#[cfg(test)]
mod tests {
    use amaru_kernel::{
        BlockHeight, EraHistory, IsHeader, any_headers_chain_with_root,
        cardano::network_block::{EncodedTestBlock, make_encoded_chain},
        utils::tests::run_strategy,
    };
    use amaru_ouroboros_traits::{WriteChainStore, in_memory_chain_store::InMemoryChainStore};

    use super::*;

    fn make_chain(len: usize, root: Point) -> Vec<EncodedTestBlock> {
        make_encoded_chain(run_strategy(any_headers_chain_with_root(len, root)), &EraHistory::default())
    }

    fn store_headers_and_blocks(store: &InMemoryChainStore, blocks: &[EncodedTestBlock]) {
        for block in blocks {
            store.store_header(&block.header).unwrap();
            store.store_block(&block.header.hash(), &block.raw).unwrap();
        }
    }

    /// A best chain of 10 blocks whose anchor is the block at index 3.
    /// Only the blocks up to the anchor are stored: the blocks after the anchor are missing.
    fn store_with_missing_blocks_after_anchor() -> (InMemoryChainStore, Vec<EncodedTestBlock>) {
        let chain = make_chain(10, Point::new(NetworkPoint::Origin, BlockHeight::from(0)));
        let store = InMemoryChainStore::new();
        store.set_anchor_point(&chain[3].header.point()).unwrap();
        for block in &chain {
            store.store_header(&block.header).unwrap();
            store.roll_forward_chain(&block.header.point()).unwrap();
        }
        for block in &chain[..=3] {
            store.store_block(&block.header.hash(), &block.raw).unwrap();
        }
        (store, chain)
    }

    fn points(chain: &[EncodedTestBlock], range: std::ops::Range<usize>) -> Vec<Point> {
        chain[range].iter().map(|block| block.header.point()).collect()
    }

    #[test]
    fn the_first_page_refuses_a_range_with_missing_blocks_after_the_anchor() {
        let (store, chain) = store_with_missing_blocks_after_anchor();

        let page =
            block_points_page(&store, &chain[0].header.point(), &chain[9].header.point(), RangeRequest::First(2));

        assert_eq!((page.points, page.next), (vec![], None));
    }

    #[test]
    fn the_first_page_gives_the_anchor_that_it_used() {
        let (store, chain) = store_with_missing_blocks_after_anchor();

        let page =
            block_points_page(&store, &chain[0].header.point(), &chain[3].header.point(), RangeRequest::First(2));

        assert_eq!(
            page,
            BlockPointsPage {
                points: points(&chain, 0..2),
                next: Some(chain[2].header.point()),
                anchor: chain[3].header.point()
            }
        );
    }

    #[test]
    fn a_next_page_does_not_read_the_part_after_the_anchor_when_it_ends_before_it() {
        let (store, chain) = store_with_missing_blocks_after_anchor();
        let anchor = chain[3].header.point();

        let request = RangeRequest::Next { page_size: 2, anchor };
        let page = block_points_page(&store, &chain[0].header.point(), &chain[9].header.point(), request);

        assert_eq!(page, BlockPointsPage { points: points(&chain, 0..2), next: Some(chain[2].header.point()), anchor });
    }

    #[test]
    fn a_next_page_that_reaches_the_anchor_reads_the_part_after_it() {
        let (store, chain) = store_with_missing_blocks_after_anchor();

        let request = RangeRequest::Next { page_size: 10, anchor: chain[3].header.point() };
        let page = block_points_page(&store, &chain[0].header.point(), &chain[9].header.point(), request);

        assert_eq!((page.points, page.next), (vec![], None));
    }

    #[test]
    fn the_pages_of_a_range_keep_the_anchor_of_the_first_page_when_the_anchor_moves() {
        let (store, chain) = store_with_missing_blocks_after_anchor();
        store_headers_and_blocks(&store, &chain[4..]);
        let (first, last) = (chain[0].header.point(), chain[9].header.point());

        let mut page = block_points_page(&store, &first, &last, RangeRequest::First(3));
        let mut pages = page.points.clone();
        store.set_anchor_point(&chain[8].header.point()).unwrap();
        while let Some(next) = page.next {
            let request = RangeRequest::Next { page_size: 3, anchor: page.anchor };
            page = block_points_page(&store, &next, &last, request);
            assert_eq!(page.anchor, chain[3].header.point());
            pages.extend(page.points.clone());
        }

        assert_eq!(pages, points(&chain, 0..10));
    }

    #[test]
    fn the_pages_of_a_range_on_a_losing_branch_are_served_after_the_anchor_passes_its_fork_point() {
        // c0 -> c1 -> c2 -> c3 -> c4 -> c5      the best chain, with the anchor at c2
        //               \
        //                -> b0 -> b1 -> b2     the range, from c2 to b2
        let main = make_chain(6, Point::new(NetworkPoint::Origin, BlockHeight::from(0)));
        let branch = make_chain(3, main[2].header.point());
        let store = InMemoryChainStore::new();
        store.set_anchor_point(&main[2].header.point()).unwrap();
        store_headers_and_blocks(&store, &main);
        for block in &main {
            store.roll_forward_chain(&block.header.point()).unwrap();
        }
        store_headers_and_blocks(&store, &branch);
        let (first, last) = (main[2].header.point(), branch[2].header.point());

        let mut page = block_points_page(&store, &first, &last, RangeRequest::First(2));
        let mut pages = page.points.clone();
        store.set_anchor_point(&main[5].header.point()).unwrap();
        while let Some(next) = page.next {
            let request = RangeRequest::Next { page_size: 2, anchor: page.anchor };
            page = block_points_page(&store, &next, &last, request);
            pages.extend(page.points.clone());
        }

        let expected = [main[2].header.point()].into_iter().chain(points(&branch, 0..3)).collect::<Vec<_>>();
        assert_eq!(pages, expected);
    }
}
