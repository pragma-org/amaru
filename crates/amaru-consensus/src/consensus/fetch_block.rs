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
use crate::consensus::block_effects::FetchBlockEffect;
use crate::consensus::span::adopt_current_span;
use crate::consensus::validate_block::ValidateBlockEvent;
use crate::consensus::{ValidateHeaderEvent, ValidationFailed};
use amaru_kernel::{Point, RawBlock, peer::Peer};
use amaru_ouroboros_traits::IsHeader;
use async_trait::async_trait;
use pure_stage::{Effects, StageRef};
use tracing::{Level, instrument};

type State = (StageRef<ValidateBlockEvent>, StageRef<ValidationFailed>);

/// This stages fetches the full block from a peer after its header has been validated.
/// It then sends the full block to the downstream stage for validation and storage.
#[instrument(
    level = Level::TRACE,
    skip_all,
    name = "stage.fetch_block",
)]
pub async fn stage(
    (downstream, validation_errors): State,
    msg: ValidateHeaderEvent,
    eff: Effects<ValidateHeaderEvent>,
) -> State {
    adopt_current_span(&msg);
    match msg {
        ValidateHeaderEvent::Validated { peer, header, span } => {
            let point = header.point();
            match eff
                .external(FetchBlockEffect::new(peer.clone(), point.clone()))
                .await
            {
                Ok(block) => {
                    let block = RawBlock::from(&*block);
                    eff.send(
                        &downstream,
                        ValidateBlockEvent::Validated {
                            peer,
                            header,
                            block,
                            span,
                        },
                    )
                    .await
                }
                Err(e) => {
                    eff.send(&validation_errors, ValidationFailed::new(&peer, e))
                        .await
                }
            }
        }
        ValidateHeaderEvent::Rollback {
            peer,
            rollback_point,
            span,
            ..
        } => {
            eff.send(
                &downstream,
                ValidateBlockEvent::Rollback {
                    peer,
                    rollback_point,
                    span,
                },
            )
            .await
        }
    }
    (downstream, validation_errors)
}

/// A trait for fetching blocks from peers.
#[async_trait]
pub trait BlockFetcher {
    async fn fetch_block(&self, peer: &Peer, point: &Point) -> Result<Vec<u8>, ConsensusError>;
}
