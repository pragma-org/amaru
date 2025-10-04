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

use crate::consensus::effects::{BaseOps, ConsensusOps, LedgerOps, MetricsOps};
use crate::consensus::errors::{ConsensusError, ProcessingFailed, ValidationFailed};
use crate::consensus::events::{BlockValidationResult, ValidateBlockEvent};
use crate::consensus::span::adopt_current_span;
use amaru_ouroboros_traits::IsHeader;
use anyhow::anyhow;
use pure_stage::StageRef;
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
    eff: impl ConsensusOps,
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

            match eff.ledger().validate(&peer, &point, block.clone()).await {
                Ok(Ok(metrics)) => {
                    eff.metrics().record(metrics.into()).await;
                    eff.base()
                        .send(
                            &downstream,
                            BlockValidationResult::BlockValidated {
                                peer,
                                header,
                                span: span.clone(),
                            },
                        )
                        .await
                }
                Ok(Err(err)) => {
                    error!(?err, "Failed to validate a block");
                    eff.base()
                        .send(
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
                    eff.base()
                        .send(
                            &processing_errors,
                            ProcessingFailed::new(&peer, anyhow!(err)),
                        )
                        .await;
                }
            }
        }
        ValidateBlockEvent::Rollback {
            peer,
            rollback_header,
            span,
            ..
        } => {
            if let Err(err) = eff.ledger().rollback(&peer, &rollback_header).await {
                error!(?err, "Failed to rollback");
                eff.base().send(&processing_errors, err).await;
            } else {
                eff.base()
                    .send(
                        &downstream,
                        BlockValidationResult::RolledBackTo {
                            peer,
                            rollback_header,
                            span,
                        },
                    )
                    .await
            }
        }
    };
    (downstream, validation_errors, processing_errors)
}
