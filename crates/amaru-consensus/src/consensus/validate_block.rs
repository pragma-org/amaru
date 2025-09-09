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
use crate::consensus::span::adopt_current_span;
use crate::consensus::{ProcessingFailed, ValidationFailed};
use amaru_kernel::peer::Peer;
use amaru_kernel::{Header, Point, RawBlock};
use amaru_ouroboros_traits::can_validate_blocks::BlockValidationError;
use amaru_ouroboros_traits::{CanValidateBlocks, IsHeader};
use pure_stage::{Effects, ExternalEffect, ExternalEffectAPI, Resources, StageRef};
use serde::{Deserialize, Serialize};
use std::fmt::{Debug, Formatter};
use std::sync::Arc;
use tracing::{Level, Span, error, instrument};

type State = (
    StageRef<BlockValidationResult>,
    StageRef<ValidationFailed>,
    StageRef<ProcessingFailed>,
);

#[instrument(
        level = Level::TRACE,
        skip_all,
        name = "stage.ledger"
)]
pub async fn stage(
    (downstream, validation_errors, processing_errors): State,
    msg: ValidateBlockEvent,
    eff: Effects<ValidateBlockEvent>,
) -> State {
    adopt_current_span(&msg);
    match msg {
        ValidateBlockEvent::Validated {
            header,
            block,
            span,
            peer,
            ..
        } => {
            let point = header.point();

            match eff
                .external(ValidateBlockEffect::new(&peer, &point, block.clone()))
                .await
            {
                Ok(Ok(block_height)) => {
                    eff.send(
                        &downstream,
                        BlockValidationResult::BlockValidated {
                            peer,
                            header,
                            block,
                            span: span.clone(),
                            block_height,
                        },
                    )
                    .await
                }
                Ok(Err(err)) => {
                    error!(?err, "Failed to validate a block");
                    eff.send(
                        &validation_errors,
                        ValidationFailed::new(
                            &peer,
                            ConsensusError::InvalidBlock {
                                peer: peer.clone(),
                                point: header.point(),
                            },
                        ),
                    )
                    .await;
                }
                Err(err) => {
                    error!(?err, "Failed to roll forward block");
                    eff.send(
                        &processing_errors,
                        ProcessingFailed::new(&peer, err.to_anyhow()),
                    )
                    .await;
                }
            }
        }
        ValidateBlockEvent::Rollback {
            peer,
            rollback_point,
            span,
            ..
        } => {
            if let Err(err) = eff
                .external(RollbackBlockEffect::new(&peer, &rollback_point))
                .await
            {
                error!(?err, "Failed to rollback");
                eff.send(&processing_errors, err).await;
            } else {
                eff.send(
                    &downstream,
                    BlockValidationResult::RolledBackTo {
                        peer,
                        rollback_point,
                        span,
                    },
                )
                .await
            }
        }
    };
    (downstream, validation_errors, processing_errors)
}

pub type ResourceBlockValidation = Arc<dyn CanValidateBlocks + Send + Sync>;

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ValidateBlockEffect {
    peer: Peer,
    point: Point,
    block: RawBlock,
}

impl ValidateBlockEffect {
    pub fn new(peer: &Peer, point: &Point, block: RawBlock) -> Self {
        Self {
            peer: peer.clone(),
            point: point.clone(),
            block: block.clone(),
        }
    }
}

impl ExternalEffect for ValidateBlockEffect {
    #[expect(clippy::expect_used)]
    fn run(
        self: Box<Self>,
        resources: Resources,
    ) -> pure_stage::BoxFuture<'static, Box<dyn pure_stage::SendData>> {
        Box::pin(async move {
            let validator = resources
                .get::<ResourceBlockValidation>()
                .expect("ValidateBlockEffect requires a HasBlockValidation resource")
                .clone();
            let result: <Self as ExternalEffectAPI>::Response =
                validator.roll_forward_block(&self.point, &self.block);
            Box::new(result) as Box<dyn pure_stage::SendData>
        })
    }
}

impl ExternalEffectAPI for ValidateBlockEffect {
    type Response = Result<Result<u64, BlockValidationError>, BlockValidationError>;
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RollbackBlockEffect {
    peer: Peer,
    point: Point,
}

impl RollbackBlockEffect {
    pub fn new(peer: &Peer, point: &Point) -> Self {
        Self {
            peer: peer.clone(),
            point: point.clone(),
        }
    }
}

impl ExternalEffect for RollbackBlockEffect {
    #[expect(clippy::expect_used)]
    fn run(
        self: Box<Self>,
        resources: Resources,
    ) -> pure_stage::BoxFuture<'static, Box<dyn pure_stage::SendData>> {
        Box::pin(async move {
            let validator = resources
                .get::<ResourceBlockValidation>()
                .expect("ValidateBlockEffect requires a HasBlockValidation resource")
                .clone();
            let result: <Self as ExternalEffectAPI>::Response = validator
                .rollback_block(&self.point)
                .map_err(|e| ProcessingFailed::new(&self.peer, e.to_anyhow()));
            Box::new(result) as Box<dyn pure_stage::SendData>
        })
    }
}

impl ExternalEffectAPI for RollbackBlockEffect {
    type Response = anyhow::Result<(), ProcessingFailed>;
}

#[derive(Clone, Serialize, Deserialize)]
pub enum ValidateBlockEvent {
    Validated {
        peer: Peer,
        header: Header,
        #[serde(skip, default = "default_block")]
        block: RawBlock,
        #[serde(skip, default = "Span::none")]
        span: Span,
    },
    Rollback {
        peer: Peer,
        rollback_point: Point,
        #[serde(skip, default = "Span::none")]
        span: Span,
    },
}

fn default_block() -> RawBlock {
    RawBlock::from(Vec::new().as_slice())
}

impl Debug for ValidateBlockEvent {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ValidateBlockEvent::Validated {
                peer,
                header,
                block,
                ..
            } => f
                .debug_struct("Validated")
                .field("peer", peer)
                .field("header", header)
                .field("block", block)
                .finish(),
            ValidateBlockEvent::Rollback {
                peer,
                rollback_point,
                ..
            } => f
                .debug_struct("Rollback")
                .field("peer", peer)
                .field("rollback_point", rollback_point)
                .finish(),
        }
    }
}

impl PartialEq for ValidateBlockEvent {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                ValidateBlockEvent::Validated {
                    peer: p1,
                    header: h1,
                    block: b1,
                    ..
                },
                ValidateBlockEvent::Validated {
                    peer: p2,
                    header: h2,
                    block: b2,
                    ..
                },
            ) => p1 == p2 && h1 == h2 && b1 == b2,
            (
                ValidateBlockEvent::Rollback {
                    peer: p1,
                    rollback_point: rp1,
                    ..
                },
                ValidateBlockEvent::Rollback {
                    peer: p2,
                    rollback_point: rp2,
                    ..
                },
            ) => p1 == p2 && rp1 == rp2,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BlockValidationResult {
    BlockValidated {
        peer: Peer,
        header: Header,
        #[serde(skip, default = "default_block")]
        block: RawBlock,
        #[serde(skip, default = "Span::none")]
        span: Span,
        block_height: u64,
    },
    BlockValidationFailed {
        peer: Peer,
        point: Point,
        #[serde(skip, default = "Span::none")]
        span: Span,
    },
    RolledBackTo {
        peer: Peer,
        rollback_point: Point,
        #[serde(skip, default = "Span::none")]
        span: Span,
    },
}

impl PartialEq for BlockValidationResult {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                BlockValidationResult::BlockValidated {
                    peer: p1,
                    header: hd1,
                    block: b1,
                    block_height: bh1,
                    ..
                },
                BlockValidationResult::BlockValidated {
                    peer: p2,
                    header: hd2,
                    block: b2,
                    block_height: bh2,
                    ..
                },
            ) => p1 == p2 && hd1 == hd2 && b1 == b2 && bh1 == bh2,
            (
                BlockValidationResult::BlockValidationFailed {
                    peer: p1,
                    point: pt1,
                    ..
                },
                BlockValidationResult::BlockValidationFailed {
                    peer: p2,
                    point: pt2,
                    ..
                },
            ) => p1 == p2 && pt1 == pt2,
            (
                BlockValidationResult::RolledBackTo {
                    peer: p1,
                    rollback_point: rp1,
                    ..
                },
                BlockValidationResult::RolledBackTo {
                    peer: p2,
                    rollback_point: rp2,
                    ..
                },
            ) => p1 == p2 && rp1 == rp2,
            _ => false,
        }
    }
}
