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

use crate::{
    effects::{BaseOps, ConsensusOps, MetricsOps},
    errors::{ConsensusError, ProcessingFailed, ValidationFailed},
    events::{DecodedChainSyncEvent, ValidateBlockEvent},
    span::HasSpan,
};
use amaru_kernel::IsHeader;
use anyhow::anyhow;
use pure_stage::StageRef;
use tracing::{Instrument, Span, error};
use tracing_opentelemetry::OpenTelemetrySpanExt;

type State = (
    StageRef<DecodedChainSyncEvent>,
    StageRef<ValidationFailed>,
    StageRef<ProcessingFailed>,
);

pub fn stage(
    (downstream, validation_errors, processing_errors): State,
    msg: ValidateBlockEvent,
    eff: impl ConsensusOps,
) -> impl Future<Output = State> {
    let span = tracing::trace_span!(parent: msg.span(), "chain_sync.validate_block");
    async move {
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
                    .ledger()
                    .validate_block(&peer, &point, block.raw_block(), Span::current().context())
                    .await
                {
                    Ok(Ok(metrics)) => {
                        eff.metrics().record(metrics.into()).await;
                        eff.base()
                            .send(
                                &downstream,
                                DecodedChainSyncEvent::RollForward {
                                    peer,
                                    header,
                                    span: span.clone(),
                                },
                            )
                            .await
                    }
                    Ok(Err(err)) => {
                        error!(?err, %point, "chain_sync.validate_block.failed");
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
                        error!(?err, %point, "chain_sync.validate_block.failed");
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
                if let Err(err) = eff
                    .ledger()
                    .rollback(&peer, &rollback_point, Span::current().context())
                    .await
                {
                    error!(?err, %rollback_point, "chain_sync.validate_block.rollback_failed");
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
        }
        (downstream, validation_errors, processing_errors)
    }
    .instrument(span)
}
