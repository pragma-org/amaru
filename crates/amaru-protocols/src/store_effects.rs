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

use amaru_kernel::{
    BlockHeader, HeaderHash, Point, RawBlock, protocol_parameters::GlobalParameters,
};
use amaru_ouroboros_traits::{ChainStore, Nonces, ReadOnlyChainStore, StoreError};
use pure_stage::{
    BoxFuture, Effects, ExternalEffect, ExternalEffectAPI, ExternalEffectSync, Resources, SendData,
};
use std::sync::Arc;

/// Implementation of ChainStore using pure_stage::Effects.
#[derive(Clone)]
pub struct Store<T> {
    effects: Effects<T>,
}

impl<T> Store<T> {
    pub fn new(effects: Effects<T>) -> Store<T> {
        Store { effects }
    }

    /// This function runs an external effect synchronously.
    pub fn external_sync<E: ExternalEffectSync + serde::Serialize + 'static>(
        &self,
        effect: E,
    ) -> E::Response {
        self.effects.external_sync(effect)
    }
}

impl<T> ReadOnlyChainStore<BlockHeader> for Store<T> {
    fn load_header(&self, hash: &HeaderHash) -> Option<BlockHeader> {
        self.external_sync(LoadHeaderEffect::new(*hash))
    }

    fn get_children(&self, hash: &HeaderHash) -> Vec<HeaderHash> {
        self.external_sync(GetChildrenEffect::new(*hash))
    }

    fn get_anchor_hash(&self) -> HeaderHash {
        self.external_sync(GetAnchorHashEffect::new())
    }

    fn get_best_chain_hash(&self) -> HeaderHash {
        self.external_sync(GetBestChainHashEffect::new())
    }

    fn load_block(&self, hash: &HeaderHash) -> Result<RawBlock, StoreError> {
        self.external_sync(LoadBlockEffect::new(*hash))
    }

    fn get_nonces(&self, hash: &HeaderHash) -> Option<Nonces> {
        self.external_sync(GetNoncesEffect::new(*hash))
    }

    fn has_header(&self, hash: &HeaderHash) -> bool {
        self.external_sync(HasHeaderEffect::new(*hash))
    }

    fn load_from_best_chain(&self, _point: &Point) -> Option<HeaderHash> {
        None
    }

    fn next_best_chain(&self, _point: &Point) -> Option<Point> {
        None
    }
}

impl<T: SendData + Sync> ChainStore<BlockHeader> for Store<T> {
    fn set_anchor_hash(&self, hash: &HeaderHash) -> Result<(), StoreError> {
        self.external_sync(SetAnchorHashEffect::new(*hash))
    }

    fn set_best_chain_hash(&self, hash: &HeaderHash) -> Result<(), StoreError> {
        self.external_sync(SetBestChainHashEffect::new(*hash))
    }

    fn store_header(&self, header: &BlockHeader) -> Result<(), StoreError> {
        self.external_sync(StoreHeaderEffect::new(header.clone()))
    }

    fn store_block(&self, hash: &HeaderHash, block: &RawBlock) -> Result<(), StoreError> {
        self.external_sync(StoreBlockEffect::new(hash, block.clone()))
    }

    fn put_nonces(&self, header: &HeaderHash, nonces: &Nonces) -> Result<(), StoreError> {
        self.external_sync(PutNoncesEffect::new(*header, nonces.clone()))
    }

    fn roll_forward_chain(&self, point: &Point) -> Result<(), StoreError> {
        self.external_sync(RollForwardChainEffect::new(*point))
    }

    fn rollback_chain(&self, point: &Point) -> Result<usize, StoreError> {
        self.external_sync(RollBackChainEffect::new(*point))
    }
}

// EXTERNAL EFFECTS DEFINITIONS

pub type ResourceHeaderStore = Arc<dyn ChainStore<BlockHeader>>;
pub type ResourceParameters = GlobalParameters;

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct StoreHeaderEffect {
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
            let store = resources
                .get::<ResourceHeaderStore>()
                .expect("StoreHeaderEffect requires a chain store")
                .clone();
            store.store_header(&self.header)
        })
    }
}

impl ExternalEffectAPI for StoreHeaderEffect {
    type Response = Result<(), StoreError>;
}

impl ExternalEffectSync for StoreHeaderEffect {}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct StoreBlockEffect {
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
            let store = resources
                .get::<ResourceHeaderStore>()
                .expect("StoreBlockEffect requires a chain store")
                .clone();
            store.store_block(&self.hash, &self.block)
        })
    }
}

impl ExternalEffectAPI for StoreBlockEffect {
    type Response = Result<(), StoreError>;
}

impl ExternalEffectSync for StoreBlockEffect {}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct SetAnchorHashEffect {
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
            let store = resources
                .get::<ResourceHeaderStore>()
                .expect("SetAnchorHashEffect requires a chain store")
                .clone();
            store.set_anchor_hash(&self.hash)
        })
    }
}

impl ExternalEffectAPI for SetAnchorHashEffect {
    type Response = Result<(), StoreError>;
}

impl ExternalEffectSync for SetAnchorHashEffect {}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct SetBestChainHashEffect {
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
            let store = resources
                .get::<ResourceHeaderStore>()
                .expect("SetBestChainHashEffect requires a chain store")
                .clone();
            store.set_best_chain_hash(&self.hash)
        })
    }
}

impl ExternalEffectAPI for SetBestChainHashEffect {
    type Response = Result<(), StoreError>;
}

impl ExternalEffectSync for SetBestChainHashEffect {}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct PutNoncesEffect {
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
            let store = resources
                .get::<ResourceHeaderStore>()
                .expect("PutNoncesEffect requires a chain store")
                .clone();
            store.put_nonces(&self.hash, &self.nonces)
        })
    }
}

impl ExternalEffectAPI for PutNoncesEffect {
    type Response = Result<(), StoreError>;
}

impl ExternalEffectSync for PutNoncesEffect {}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct HasHeaderEffect {
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
            let store = resources
                .get::<ResourceHeaderStore>()
                .expect("HasHeaderEffect requires a chain store")
                .clone();
            store.has_header(&self.hash)
        })
    }
}

impl ExternalEffectAPI for HasHeaderEffect {
    type Response = bool;
}

impl ExternalEffectSync for HasHeaderEffect {}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct LoadHeaderEffect {
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
            let store = resources
                .get::<ResourceHeaderStore>()
                .expect("LoadHeaderEffect requires a chain store")
                .clone();
            store.load_header(&self.hash)
        })
    }
}

impl ExternalEffectAPI for LoadHeaderEffect {
    type Response = Option<BlockHeader>;
}

impl ExternalEffectSync for LoadHeaderEffect {}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct GetChildrenEffect {
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
            let store = resources
                .get::<ResourceHeaderStore>()
                .expect("GetChildrenEffect requires a chain store")
                .clone();
            store.get_children(&self.hash)
        })
    }
}

impl ExternalEffectAPI for GetChildrenEffect {
    type Response = Vec<HeaderHash>;
}

impl ExternalEffectSync for GetChildrenEffect {}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct GetAnchorHashEffect;

impl GetAnchorHashEffect {
    pub fn new() -> Self {
        Self {}
    }
}

impl ExternalEffect for GetAnchorHashEffect {
    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap_sync({
            let store = resources
                .get::<ResourceHeaderStore>()
                .expect("GetAnchorHashEffect requires a chain store")
                .clone();
            store.get_anchor_hash()
        })
    }
}

impl ExternalEffectAPI for GetAnchorHashEffect {
    type Response = HeaderHash;
}

impl ExternalEffectSync for GetAnchorHashEffect {}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct GetBestChainHashEffect;

impl GetBestChainHashEffect {
    pub fn new() -> Self {
        Self {}
    }
}

impl ExternalEffect for GetBestChainHashEffect {
    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap_sync({
            let store = resources
                .get::<ResourceHeaderStore>()
                .expect("GetBestChainHashEffect requires a chain store")
                .clone();
            store.get_best_chain_hash()
        })
    }
}

impl ExternalEffectAPI for GetBestChainHashEffect {
    type Response = HeaderHash;
}

impl ExternalEffectSync for GetBestChainHashEffect {}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct LoadBlockEffect {
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
            let store = resources
                .get::<ResourceHeaderStore>()
                .expect("LoadBlockEffect requires a chain store")
                .clone();
            store.load_block(&self.hash)
        })
    }
}

impl ExternalEffectAPI for LoadBlockEffect {
    type Response = Result<RawBlock, StoreError>;
}

impl ExternalEffectSync for LoadBlockEffect {}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct GetNoncesEffect {
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
            let store = resources
                .get::<ResourceHeaderStore>()
                .expect("GetNoncesEffect requires a chain store")
                .clone();
            store.get_nonces(&self.hash)
        })
    }
}

impl ExternalEffectAPI for GetNoncesEffect {
    type Response = Option<Nonces>;
}

impl ExternalEffectSync for GetNoncesEffect {}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct RollForwardChainEffect {
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
            let store = resources
                .get::<ResourceHeaderStore>()
                .expect("RollForwardChainEffect requires a chain store")
                .clone();
            store.roll_forward_chain(&self.point)
        })
    }
}

impl ExternalEffectAPI for RollForwardChainEffect {
    type Response = Result<(), StoreError>;
}

impl ExternalEffectSync for RollForwardChainEffect {}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct RollBackChainEffect {
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
            let store = resources
                .get::<ResourceHeaderStore>()
                .expect("RollBackChainEffect requires a chain store")
                .clone();
            store.rollback_chain(&self.point)
        })
    }
}

impl ExternalEffectAPI for RollBackChainEffect {
    type Response = Result<usize, StoreError>;
}

impl ExternalEffectSync for RollBackChainEffect {}
