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
use amaru_kernel::{Header, Point, protocol_parameters::GlobalParameters};
use amaru_ouroboros::{IsHeader, Praos};
use pure_stage::{BoxFuture, Effects, ExternalEffect, ExternalEffectAPI, Resources};
use std::sync::Arc;
use tokio::sync::Mutex;

pub type ResourceHeaderStore = Arc<Mutex<dyn super::store::ChainStore<Header>>>;
pub type ResourceParameters = GlobalParameters;

pub trait StorageEffect {
    fn store_header(
        &self,
        header: Header,
        point: Point,
    ) -> BoxFuture<'static, Result<(), ConsensusError>>;
    fn evolve_nonce(
        &self,
        header: Header,
    ) -> BoxFuture<'static, Result<amaru_ouroboros::Nonces, super::store::NoncesError>>;
}

impl<M> StorageEffect for Effects<M> {
    fn store_header(
        &self,
        header: Header,
        point: Point,
    ) -> BoxFuture<'static, Result<(), ConsensusError>> {
        self.external(StoreHeaderEffect { header, point })
    }

    fn evolve_nonce(
        &self,
        header: Header,
    ) -> BoxFuture<'static, Result<amaru_ouroboros::Nonces, super::store::NoncesError>> {
        self.external(EvolveNonceEffect { header })
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoreHeaderEffect {
    header: Header,
    point: Point,
}

impl ExternalEffect for StoreHeaderEffect {
    #[expect(clippy::expect_used)]
    fn run(
        self: Box<Self>,
        resources: Resources,
    ) -> pure_stage::BoxFuture<'static, Box<dyn pure_stage::SendData>> {
        Self::wrap(async move {
            let store = resources
                .get::<ResourceHeaderStore>()
                .expect("StoreHeaderEffect requires a chain store")
                .clone();
            let mut store = store.lock().await;
            store
                .store_header(&self.header.hash(), &self.header)
                .map_err(|e| ConsensusError::StoreHeaderFailed(self.point.clone(), e))
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

impl ExternalEffect for EvolveNonceEffect {
    #[expect(clippy::expect_used)]
    fn run(
        self: Box<Self>,
        resources: Resources,
    ) -> pure_stage::BoxFuture<'static, Box<dyn pure_stage::SendData>> {
        Self::wrap(async move {
            let store = resources
                .get::<ResourceHeaderStore>()
                .expect("EvolveNonceEffect requires a chain store")
                .clone();
            let mut store = store.lock().await;
            let global_parameters = resources
                .get::<ResourceParameters>()
                .expect("EvolveNonceEffect requires global parameters");
            store.evolve_nonce(&self.header, &global_parameters)
        })
    }
}

impl ExternalEffectAPI for EvolveNonceEffect {
    type Response = Result<amaru_ouroboros::Nonces, super::store::NoncesError>;
}
