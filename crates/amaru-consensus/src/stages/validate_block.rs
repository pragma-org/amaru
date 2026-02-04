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
use amaru_kernel::{Block, BlockHeader, IsHeader, Peer, Point};
use pure_stage::StageRef;
use tracing::{Instrument, Span, debug, error};
use tracing_opentelemetry::OpenTelemetrySpanExt;

type State = (
    StageRef<DecodedChainSyncEvent>,
    StageRef<ValidationFailed>,
    StageRef<ProcessingFailed>,
);

pub fn stage(
    state: State,
    msg: ValidateBlockEvent,
    eff: impl ConsensusOps,
) -> impl Future<Output = State> {
    let span = tracing::trace_span!(parent: msg.span(), "chain_sync.validate_block");

    async move {
        process_event(msg, eff, &state).await;
        state
    }
    .instrument(span)
}

/// Process a single ValidateBlockEvent, either rolling forward or rolling back.
async fn process_event(msg: ValidateBlockEvent, eff: impl ConsensusOps, state: &State) {
    let (_, validation_errors, _) = state;
    match msg {
        ValidateBlockEvent::Validated {
            header,
            block,
            span,
            peer,
            ..
        } => {
            let point = header.point();

            match block.decode_block() {
                Ok(block) => {
                    roll_forward(&eff, state, header, span, peer, point, block).await;
                }
                Err(err) => {
                    fail_with(
                        "decode_block",
                        &eff,
                        validation_errors,
                        peer,
                        point,
                        err.into(),
                    )
                    .await;
                }
            };
        }
        ValidateBlockEvent::Rollback {
            peer,
            rollback_point,
            span,
            ..
        } => {
            rollback(&eff, state, peer, rollback_point, span).await;
        }
    }
}

/// Roll forward the block: decode it and apply it to the current ledger state.
async fn roll_forward(
    eff: &impl ConsensusOps,
    state: &State,
    header: BlockHeader,
    span: Span,
    peer: Peer,
    point: Point,
    block: Block,
) {
    let (downstream, validation_errors, processing_errors) = state;
    match eff
        .ledger()
        .validate_block(&peer, &point, block, Span::current().context())
        .await
    {
        Ok(Ok(metrics)) => {
            eff.metrics().record(metrics.into()).await;
            eff.base()
                .send(
                    downstream,
                    DecodedChainSyncEvent::RollForward {
                        peer,
                        header,
                        span: span.clone(),
                    },
                )
                .await
        }
        Ok(Err(err)) => {
            fail_with(
                "validate_block",
                eff,
                validation_errors,
                peer,
                point,
                err.into(),
            )
            .await;
        }
        Err(err) => {
            error_with(
                "validate_block",
                eff,
                processing_errors,
                peer,
                point,
                err.into(),
            )
            .await;
        }
    }
}

/// Rollback the ledger to the given point.
async fn rollback(
    eff: &impl ConsensusOps,
    state: &State,
    peer: Peer,
    rollback_point: Point,
    span: Span,
) {
    let (downstream, _, processing_errors) = state;
    if let Err(err) = eff
        .ledger()
        .rollback(&peer, &rollback_point, Span::current().context())
        .await
    {
        error_with(
            "rollback_block",
            eff,
            processing_errors,
            peer,
            rollback_point,
            err.into(),
        )
        .await;
    } else {
        eff.base()
            .send(
                downstream,
                DecodedChainSyncEvent::Rollback {
                    peer,
                    rollback_point,
                    span,
                },
            )
            .await
    }
}

/// Handle a failure by logging and sending a ValidationFailed event downstream.
async fn fail_with(
    failure_type: &str,
    eff: &impl ConsensusOps,
    validation_errors: &StageRef<ValidationFailed>,
    peer: Peer,
    point: Point,
    err: anyhow::Error,
) {
    debug!(?err, %point, "chain_sync.{failure_type}.failure");
    eff.base()
        .send(
            validation_errors,
            ValidationFailed::new(
                &peer,
                ConsensusError::InvalidBlock {
                    peer: peer.clone(),
                    point,
                },
            ),
        )
        .await;
}

/// Handle a processing error by logging and sending a ProcessingFailed event downstream.
async fn error_with(
    error_type: &str,
    eff: &impl ConsensusOps,
    processing_errors: &StageRef<ProcessingFailed>,
    peer: Peer,
    point: Point,
    err: anyhow::Error,
) {
    error!(?err, %point, "chain_sync.{error_type}.error");
    eff.base()
        .send(processing_errors, ProcessingFailed::new(&peer, err))
        .await;
}
