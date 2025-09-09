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

use crate::consensus::errors::{ConsensusError, ProcessingFailed, ValidationFailed};
use crate::consensus::events::{BlockValidationResult, ValidateBlockEvent};
use crate::consensus::span::adopt_current_span;
use amaru_kernel::peer::Peer;
use amaru_kernel::{Point, RawBlock};
use amaru_ouroboros_traits::can_validate_blocks::BlockValidationError;
use amaru_ouroboros_traits::{CanValidateBlocks, IsHeader};
use pure_stage::{Effects, ExternalEffect, ExternalEffectAPI, Resources, StageRef};
use std::fmt::Debug;
use std::sync::Arc;
use tracing::{Level, error, instrument};

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
                .expect("ValidateBlockEffect requires a CanValidateBlock resource")
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
