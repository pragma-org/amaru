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

use crate::{ConsensusError, span::adopt_current_span};
use amaru_kernel::{Hash, Header, MintedHeader, Point, cbor};
use async_trait::async_trait;
use tracing::{Level, instrument};

use super::{ChainSyncEvent, DecodedChainSyncEvent, ValidationFailed};
use pure_stage::{Effects, Stage, StageRef};

#[instrument(
        level = Level::TRACE,
        skip_all,
        name = "consensus.receive_header",
        fields(
            point.slot = %point.slot_or_default(),
            point.hash = %Hash::<32>::from(point),
        )
)]
pub fn receive_header(point: &Point, raw_header: &[u8]) -> Result<Header, ConsensusError> {
    let header: MintedHeader<'_> =
        cbor::decode(raw_header).map_err(|reason| ConsensusError::CannotDecodeHeader {
            point: point.clone(),
            header: raw_header.into(),
            reason: reason.to_string(),
        })?;

    Ok(Header::from(header))
}

#[derive(Clone)]
pub struct ReceiveHeader {
    downstream: StageRef<DecodedChainSyncEvent>,
    errors: StageRef<ValidationFailed>,
}

impl ReceiveHeader {
    pub fn new(
        downstream: impl AsRef<StageRef<DecodedChainSyncEvent>>,
        errors: impl AsRef<StageRef<ValidationFailed>>,
    ) -> Self {
        Self {
            downstream: downstream.as_ref().clone(),
            errors: errors.as_ref().clone(),
        }
    }
}

#[async_trait]
impl Stage<ChainSyncEvent, ()> for ReceiveHeader {
    fn initial_state(&self) {}

    #[instrument(
    level = Level::TRACE,
    skip_all,
    name = "stage.receive_header"
    )]
    async fn run(&self, _state: (), msg: ChainSyncEvent, eff: Effects<ChainSyncEvent>) -> () {
        adopt_current_span(&msg);
        match msg {
            ChainSyncEvent::RollForward {
                peer,
                point,
                raw_header,
                span,
            } => {
                let header = match receive_header(&point, raw_header.as_slice()) {
                    Ok(header) => header,
                    Err(error) => {
                        tracing::error!(%error, %point, %peer, "Failed to decode header");
                        eff.send(&self.errors, ValidationFailed::new(peer, error))
                            .await;
                        return;
                    }
                };
                eff.send(
                    &self.downstream,
                    DecodedChainSyncEvent::RollForward {
                        peer,
                        point,
                        header,
                        span,
                    },
                )
                .await;
            }
            ChainSyncEvent::Rollback {
                peer,
                rollback_point,
                span,
            } => {
                eff.send(
                    &self.downstream,
                    DecodedChainSyncEvent::Rollback {
                        peer,
                        rollback_point,
                        span,
                    },
                )
                .await
            }
            ChainSyncEvent::CaughtUp { peer, span } => {
                eff.send(
                    &self.downstream,
                    DecodedChainSyncEvent::CaughtUp { peer, span },
                )
                .await
            }
        }
    }
}
