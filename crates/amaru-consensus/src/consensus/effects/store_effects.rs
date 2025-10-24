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

use amaru_kernel::{protocol_parameters::GlobalParameters, BlockHeader, HeaderHash, Point, RawBlock};
use amaru_ouroboros_traits::{ChainStore, Nonces, ReadOnlyChainStore, StoreError};
use pure_stage::{BoxFuture, Effects, ExternalEffect, ExternalEffectAPI, Resources, SendData};
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
    pub fn external_sync<E: ExternalEffectAPI + 'static>(&self, effect: E) -> E::Response
    where
        T: SendData + Sync,
    {
        self.effects.external_sync(effect)
    }
}

impl<T: SendData + Sync> ReadOnlyChainStore<BlockHeader> for Store<T> {
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
        todo!()
    }

    fn roll_back_chain(&self, point: &Point) -> Result<usize, StoreError> {
        todo!()
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
        Self::wrap(async move {
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
        Self::wrap(async move {
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
        Self::wrap(async move {
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
        Self::wrap(async move {
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
        Self::wrap(async move {
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
        Self::wrap(async move {
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
        Self::wrap(async move {
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
        Self::wrap(async move {
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
        Self::wrap(async move {
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
        Self::wrap(async move {
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
        Self::wrap(async move {
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
