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
use amaru_kernel::Point;
use crate::consensus::errors::ProcessingFailed;
use crate::consensus::tip::HeaderTip;
use amaru_kernel::peer::Peer;
use pure_stage::{Effects, ExternalEffect, ExternalEffectAPI, Resources, SendData};
use amaru_kernel::{Header, Point};
use amaru_ouroboros_traits::IsHeader;
use anyhow::anyhow;
use async_trait::async_trait;
use pure_stage::{ExternalEffect, ExternalEffectAPI, Resources};
use serde::{Deserialize, Serialize};
use std::fmt::Display;
use std::sync::Arc;

pub type ResourceBlockFetcher = Arc<dyn BlockFetcher + Send + Sync>;

pub struct Network<'a, T>(&'a Effects<T>);

impl<'a, T> Network<'a, T> {
    pub fn new(eff: &'a Effects<T>) -> Network<'a, T> {
        Network(eff)
    }
}

pub trait NetworkOps {
    fn fetch_block(
        &self,
        peer: &Peer,
        point: &Point,
    ) -> impl Future<Output = Result<Vec<u8>, ConsensusError>> + Send;
}

impl<T: SendData + Sync> NetworkOps for Network<'_, T> {
    fn fetch_block(
        &self,
        peer: &Peer,
        point: &Point,
    ) -> impl Future<Output = Result<Vec<u8>, ConsensusError>> + Send {
        self.0.external(FetchBlockEffect::new(peer, point))
    }
}

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
    pub fn new(peer: &Peer, point: &Point) -> Self {
        Self {
            peer: peer.clone(),
            point: point.clone(),
        }
    }
}

impl ExternalEffect for FetchBlockEffect {
    #[expect(clippy::expect_used)]
    fn run(
        self: Box<Self>,
        resources: Resources,
    ) -> pure_stage::BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap(async move {
            let block_fetcher = resources
                .get::<ResourceBlockFetcher>()
                .expect("FetchBlockEffect requires a BlockFetcher")
                .clone();
            block_fetcher.fetch_block(&self.peer, &self.point).await
        })
    }
}

pub type ResourceForwardEventListener = Arc<dyn ForwardEventListener + Send + Sync>;

/// A listener interface for forward events (new headers or rollbacks).
/// These events are either caught for tests or forwarded to downstream peers (see the TcpForwardEventListener implementation).
#[async_trait]
pub trait ForwardEventListener {
    async fn send(&self, event: ForwardEvent) -> anyhow::Result<()>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ForwardEvent {
    Forward(Header),
    Backward(HeaderTip),
}

impl ForwardEvent {
    pub fn point(&self) -> Point {
        match self {
            ForwardEvent::Forward(header) => header.point(),
            ForwardEvent::Backward(tip) => tip.point(),
        }
    }
}

impl Display for ForwardEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ForwardEvent::Forward(header) => write!(f, "Forward({})", header.point()),
            ForwardEvent::Backward(tip) => write!(f, "Backward({})", tip),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForwardEventEffect {
    peer: Peer,
    event: ForwardEvent,
}

impl ForwardEventEffect {
    pub fn new(peer: &Peer, event: ForwardEvent) -> Self {
        Self {
            peer: peer.clone(),
            event,
        }
    }
}

impl ExternalEffect for ForwardEventEffect {
    #[expect(clippy::expect_used)]
    fn run(
        self: Box<Self>,
        resources: Resources,
    ) -> pure_stage::BoxFuture<'static, Box<dyn pure_stage::SendData>> {
        Box::pin(async move {
            let listener = resources
                .get::<ResourceForwardEventListener>()
                .expect("ForwardEventEffect requires a ForwardEventListener")
                .clone();

            let point = self.event.point();
            let result: <Self as ExternalEffectAPI>::Response =
                listener.send(self.event).await.map_err(|e| {
                    ProcessingFailed::new(
                        &self.peer,
                        anyhow!("Cannot send the forward event {}: {e}", &point),
                    )
                });
            Box::new(result) as Box<dyn pure_stage::SendData>
        })
    }
}

impl ExternalEffectAPI for ForwardEventEffect {
    type Response = Result<(), ProcessingFailed>;
}
