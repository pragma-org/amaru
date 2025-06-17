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

use crate::stages::PeerSession;
use crate::{schedule, stages::common::adopt_current_span};
use amaru_consensus::{consensus::ValidateHeaderEvent, peer::Peer, ConsensusError};
use amaru_kernel::{block::ValidateBlockEvent, Point};
use gasket::framework::*;
use tracing::{error, instrument};

pub type UpstreamPort = gasket::messaging::InputPort<ValidateHeaderEvent>;
pub type DownstreamPort = gasket::messaging::OutputPort<ValidateBlockEvent>;

#[derive(Stage)]
#[stage(name = "stage.fetch", unit = "ValidateHeaderEvent", worker = "Worker")]
pub struct BlockFetchStage {
    pub peer_sessions: BTreeMap<Peer, PeerSession>,
    pub upstream: UpstreamPort,
    pub downstream: DownstreamPort,
}

impl BlockFetchStage {
    pub fn new(sessions: &[PeerSession]) -> Self {
        let peer_sessions = sessions
            .iter()
            .map(|p| (p.peer.clone(), p.clone()))
            .collect::<BTreeMap<_, _>>();
        Self {
            peer_sessions,
            upstream: Default::default(),
            downstream: Default::default(),
        }
    }

    async fn handle_event(&mut self, event: ValidateHeaderEvent) -> Result<(), WorkerError> {
        match event {
            ValidateHeaderEvent::Validated { peer, point, span } => {
                let block = self.fetch_block(&peer, &point).await.map_err(|e| {
                    error!(error=%e, "failed to fetch block");
                    WorkerError::Recv
                })?;

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

    async fn fetch_block(&self, peer: &Peer, point: &Point) -> Result<Vec<u8>, ConsensusError> {
        // FIXME: should not crash if the peer is not found
        // the block should be fetched from any other valid peer
        // which is known to have it
        let peer_session = self
            .peer_sessions
            .get(peer)
            .ok_or_else(|| ConsensusError::UnknownPeer(peer.clone()))?;
        let mut session = peer_session.peer_client.lock().await;
        let client = (*session).blockfetch();
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
        adopt_current_span(unit);
        stage.handle_event(unit.clone()).await
    }
}
