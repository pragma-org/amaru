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

use crate::consensus::effects::{BaseOps, ConsensusOps, MetricsOps};
use crate::consensus::errors::{ConsensusError, ProcessingFailed, ValidationFailed};
use crate::consensus::events::{DecodedChainSyncEvent, ValidateBlockEvent};
use crate::consensus::span::HasSpan;
use amaru_ouroboros_traits::IsHeader;
use anyhow::anyhow;
use pure_stage::StageRef;
use tracing::{Level, error, span};

type State = (
    StageRef<DecodedChainSyncEvent>,
    StageRef<ValidationFailed>,
    StageRef<ProcessingFailed>,
);

pub async fn stage(
    (downstream, validation_errors, processing_errors): State,
    msg: ValidateBlockEvent,
    eff: impl ConsensusOps,
) -> State {
    let span = span!(parent: msg.span(), Level::TRACE, "stage.validate_block");
    let _entered = span.enter();

    match msg {
        ValidateBlockEvent::Validated {
            header,
            block,
            span,
            peer,
            ..
        } => {
            let point = header.point();

            match eff.ledger().validate_block(&peer, &point, block).await {
                Ok(Ok(metrics)) => {
                    eff.metrics().record(metrics.into()).await;
                    eff.base()
                        .send(
                            &downstream,
                            DecodedChainSyncEvent::RollForward {
                                peer,
                                point,
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
            rollback_point,
            span,
            ..
        } => {
            if let Err(err) = eff.ledger().rollback(&peer, &rollback_point).await {
                error!(?err, "Failed to rollback");
                eff.base().send(&processing_errors, err).await;
            } else {
                eff.base()
                    .send(
                        &downstream,
                        DecodedChainSyncEvent::Rollback {
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
