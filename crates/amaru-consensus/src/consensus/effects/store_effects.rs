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

use crate::consensus::errors::{ConsensusError, ProcessingFailed};
use crate::consensus::store::{PraosChainStore, StoreOps};
use amaru_kernel::peer::Peer;
use amaru_kernel::{Header, Point, RawBlock, protocol_parameters::GlobalParameters};
use amaru_ouroboros::{IsHeader, Praos};
use amaru_ouroboros_traits::{ChainStore, HasStakeDistribution, Nonces};
use anyhow::anyhow;
use pure_stage::{Effects, ExternalEffect, ExternalEffectAPI, Resources};
use std::sync::Arc;

pub struct Store<'a, T>(pub &'a mut Effects<T>);

impl<T> StoreOps for Store<'_, T> {
    fn store_header(
        &mut self,
        peer: &Peer,
        header: &Header,
    ) -> impl std::future::Future<Output = Result<(), ProcessingFailed>> + Send {
        self.0
            .external(StoreHeaderEffect::new(peer, header.clone()))
    }

    fn store_block(
        &mut self,
        peer: &Peer,
        point: &Point,
        block: &RawBlock,
    ) -> impl std::future::Future<Output = Result<(), ProcessingFailed>> + Send {
        self.0
            .external(StoreBlockEffect::new(peer, point, block.clone()))
    }

    fn evolve_nonce(
        &mut self,
        peer: &Peer,
        header: &Header,
    ) -> impl std::future::Future<Output = Result<Nonces, ConsensusError>> + Send {
        self.0
            .external(EvolveNonceEffect::new(peer, header.clone()))
    }
}

pub type ResourceHeaderStore = Arc<dyn ChainStore<Header>>;
pub type ResourceHeaderValidation = Arc<dyn HasStakeDistribution>;
pub type ResourceParameters = GlobalParameters;

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct StoreHeaderEffect {
    peer: Peer,
    header: Header,
}

impl StoreHeaderEffect {
    pub fn new(peer: &Peer, header: Header) -> Self {
        Self {
            peer: peer.clone(),
            header,
        }
    }
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
            store.store_header(&self.header).map_err(|e| {
                ProcessingFailed::new(
                    &self.peer,
                    anyhow!("Cannot store the header at {}: {e}", self.header.point()),
                )
            })
        })
    }
}

impl ExternalEffectAPI for StoreHeaderEffect {
    type Response = Result<(), ProcessingFailed>;
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct StoreBlockEffect {
    peer: Peer,
    point: Point,
    block: RawBlock,
}

impl StoreBlockEffect {
    pub fn new(peer: &Peer, point: &Point, block: RawBlock) -> Self {
        Self {
            peer: peer.clone(),
            point: point.clone(),
            block,
        }
    }
}

impl ExternalEffect for StoreBlockEffect {
    #[expect(clippy::expect_used)]
    fn run(
        self: Box<Self>,
        resources: Resources,
    ) -> pure_stage::BoxFuture<'static, Box<dyn pure_stage::SendData>> {
        Self::wrap(async move {
            let store = resources
                .get::<ResourceHeaderStore>()
                .expect("StoreBlockEffect requires a chain store")
                .clone();
            store
                .store_block(&self.point.hash(), &self.block)
                .map_err(|e| {
                    ProcessingFailed::new(
                        &self.peer,
                        anyhow!("Cannot store the block at {}: {e}", self.point.clone()),
                    )
                })
        })
    }
}

impl ExternalEffectAPI for StoreBlockEffect {
    type Response = Result<(), ProcessingFailed>;
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct EvolveNonceEffect {
    peer: Peer,
    header: Header,
}

impl EvolveNonceEffect {
    pub fn new(peer: &Peer, header: Header) -> Self {
        Self {
            peer: peer.clone(),
            header,
        }
    }
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
            let global_parameters = resources
                .get::<ResourceParameters>()
                .expect("EvolveNonceEffect requires global parameters");
            PraosChainStore::new(store)
                .evolve_nonce(&self.header, &global_parameters)
                .map_err(ConsensusError::NoncesError)
        })
    }
}

impl ExternalEffectAPI for EvolveNonceEffect {
    type Response = Result<Nonces, ConsensusError>;
}
