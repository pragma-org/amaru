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

use crate::ConsensusError;
use crate::consensus::store::PraosChainStore;
use amaru_kernel::{Header, Point, protocol_parameters::GlobalParameters};
use amaru_ouroboros::Praos;
use amaru_stores::chain_store::ChainStore;
use pure_stage::{ExternalEffect, ExternalEffectAPI, Resources};
use std::sync::Arc;

pub type ResourceHeaderStore = Arc<dyn ChainStore<Header>>;
pub type ResourceParameters = GlobalParameters;

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoreHeaderEffect {
    header: Header,
    point: Point,
}

impl StoreHeaderEffect {
    pub fn new(header: Header, point: Point) -> Self {
        Self { header, point }
    }
}

impl ExternalEffect for StoreHeaderEffect {
    #[expect(clippy::expect_used)]
    fn run(
        self: Box<Self>,
        resources: Resources,
    ) -> pure_stage::BoxFuture<'static, Box<dyn pure_stage::SendData>> {
        Box::pin(async move {
            let store = resources
                .get::<ResourceHeaderStore>()
                .expect("StoreHeaderEffect requires a chain store")
                .clone();
            let result: <Self as ExternalEffectAPI>::Response = store
                .store_header(&self.header)
                .map_err(|e| ConsensusError::StoreHeaderFailed(self.point.hash(), e));
            Box::new(result) as Box<dyn pure_stage::SendData>
        })
    }
}

impl ExternalEffectAPI for StoreHeaderEffect {
    type Response = Result<(), ConsensusError>;
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EvolveNonceEffect {
    header: Header,
}

impl EvolveNonceEffect {
    pub fn new(header: Header) -> Self {
        Self { header }
    }
}

impl ExternalEffect for EvolveNonceEffect {
    #[expect(clippy::expect_used)]
    fn run(
        self: Box<Self>,
        resources: Resources,
    ) -> pure_stage::BoxFuture<'static, Box<dyn pure_stage::SendData>> {
        Box::pin(async move {
            let store = resources
                .get::<ResourceHeaderStore>()
                .expect("EvolveNonceEffect requires a chain store")
                .clone();
            let global_parameters = resources
                .get::<ResourceParameters>()
                .expect("EvolveNonceEffect requires global parameters");
            let result: <Self as ExternalEffectAPI>::Response =
                PraosChainStore::new(store).evolve_nonce(&self.header, &global_parameters);
            Box::new(result) as Box<dyn pure_stage::SendData>
        })
    }
}

impl ExternalEffectAPI for EvolveNonceEffect {
    type Response = Result<amaru_ouroboros::Nonces, super::store::NoncesError>;
}
