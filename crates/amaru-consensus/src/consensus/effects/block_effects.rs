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

use crate::consensus::errors::ConsensusError;
use crate::consensus::stages::fetch_block::BlockFetcher;
use amaru_kernel::peer::Peer;
use amaru_kernel::{Point, protocol_parameters::GlobalParameters};
use pure_stage::{ExternalEffect, ExternalEffectAPI, Resources};
use std::sync::Arc;

pub type ResourceBlockFetcher = Arc<dyn BlockFetcher + Send + Sync>;
pub type ResourceParameters = GlobalParameters;

/// This effect is used to fetch a block from a peer given a point (hash + slot).
/// The effect response is either a vector of bytes representing the block,
/// or a ConsensusError if the block could not be fetched.
#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FetchBlockEffect {
    peer: Peer,
    point: Point,
}

impl ExternalEffectAPI for FetchBlockEffect {
    type Response = Result<Vec<u8>, ConsensusError>;
}

impl FetchBlockEffect {
    pub fn new(peer: Peer, point: Point) -> Self {
        Self { peer, point }
    }
}

impl ExternalEffect for FetchBlockEffect {
    #[expect(clippy::expect_used)]
    fn run(
        self: Box<Self>,
        resources: Resources,
    ) -> pure_stage::BoxFuture<'static, Box<dyn pure_stage::SendData>> {
        Self::wrap(async move {
            let block_fetcher = resources
                .get::<ResourceBlockFetcher>()
                .expect("FetchBlockEffect requires a BlockFetcher")
                .clone();
            block_fetcher.fetch_block(&self.peer, &self.point).await
        })
    }
}
