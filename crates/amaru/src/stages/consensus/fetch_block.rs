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

use std::collections::BTreeMap;

use crate::schedule;
use amaru_consensus::{
    ConsensusError, IsHeader, consensus::ValidateHeaderEvent, span::adopt_current_span,
};
use amaru_kernel::{Point, RawBlock, block::ValidateBlockEvent, peer::Peer};
use gasket::framework::*;
use pallas_network::miniprotocols::blockfetch::Client;
use tracing::{error, instrument};

pub type UpstreamPort = gasket::messaging::InputPort<ValidateHeaderEvent>;
pub type DownstreamPort = gasket::messaging::OutputPort<ValidateBlockEvent>;

#[derive(Stage)]
#[stage(name = "stage.fetch", unit = "ValidateHeaderEvent", worker = "Worker")]
pub struct BlockFetchStage {
    pub clients: BTreeMap<Peer, Client>,
    pub upstream: UpstreamPort,
    pub downstream: DownstreamPort,
}

impl BlockFetchStage {
    pub fn new(clients: Vec<(Peer, Client)>) -> Self {
        let clients = clients.into_iter().collect::<BTreeMap<_, _>>();
        Self {
            clients,
            upstream: Default::default(),
            downstream: Default::default(),
        }
    }

    async fn handle_event(&mut self, event: ValidateHeaderEvent) -> Result<(), WorkerError> {
        match event {
            ValidateHeaderEvent::Validated { peer, header, span } => {
                let point = header.point();
                let block = self.fetch_block(&peer, &point).await.map_err(|e| {
                    error!(error=%e, "failed to fetch block");
                    WorkerError::Recv
                })?;
                let block = RawBlock::from(&*block);

                self.downstream
                    .send(ValidateBlockEvent::Validated { point, block, span }.into())
                    .await
                    .map_err(|e| {
                        error!(error=%e, "failed to send event");
                        WorkerError::Send
                    })?
            }
            ValidateHeaderEvent::Rollback {
                rollback_point,
                span,
                ..
            } => self
                .downstream
                .send(
                    ValidateBlockEvent::Rollback {
                        rollback_point,
                        span,
                    }
                    .into(),
                )
                .await
                .map_err(|e| {
                    error!(error=%e, "failed to send event");
                    WorkerError::Send
                })?,
        }

        Ok(())
    }

    async fn fetch_block(&mut self, peer: &Peer, point: &Point) -> Result<Vec<u8>, ConsensusError> {
        // FIXME: should not crash if the peer is not found
        // the block should be fetched from any other valid peer
        // which is known to have it
        let client = self
            .clients
            .get_mut(peer)
            .ok_or_else(|| ConsensusError::UnknownPeer(peer.clone()))?;
        let new_point: pallas_network::miniprotocols::Point = match point.clone() {
            Point::Origin => pallas_network::miniprotocols::Point::Origin,
            Point::Specific(slot, hash) => {
                pallas_network::miniprotocols::Point::Specific(slot, hash)
            }
        };
        client
            .fetch_single(new_point)
            .await
            .map_err(|_| ConsensusError::FetchBlockFailed(point.clone()))
    }
}

pub struct Worker {}

#[async_trait::async_trait(?Send)]
impl gasket::framework::Worker<BlockFetchStage> for Worker {
    async fn bootstrap(_stage: &BlockFetchStage) -> Result<Self, WorkerError> {
        Ok(Self {})
    }

    async fn schedule(
        &mut self,
        stage: &mut BlockFetchStage,
    ) -> Result<WorkSchedule<ValidateHeaderEvent>, WorkerError> {
        schedule!(&mut stage.upstream)
    }

    #[instrument(
        level = tracing::Level::TRACE,
        name = "stage.fetch_block",
        skip_all
    )]
    async fn execute(
        &mut self,
        unit: &ValidateHeaderEvent,
        stage: &mut BlockFetchStage,
    ) -> Result<(), WorkerError> {
        let _span = adopt_current_span(unit).entered();
        stage.handle_event(unit.clone()).await
    }
}
