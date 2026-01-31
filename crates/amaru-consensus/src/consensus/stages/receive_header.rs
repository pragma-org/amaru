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

use crate::consensus::{
    effects::{BaseOps, ConsensusOps},
    errors::{ConsensusError, ProcessingFailed, ValidationFailed},
    events::{ChainSyncEvent, DecodedChainSyncEvent},
    span::HasSpan,
};
use amaru_kernel::{BlockHeader, IsHeader, from_cbor_no_leftovers};
use pure_stage::StageRef;
use tracing::{Instrument, Level, instrument};

#[derive(Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct State {
    downstream: StageRef<DecodedChainSyncEvent>,
    failures: StageRef<ValidationFailed>,
    errors: StageRef<ProcessingFailed>,
    recv_count: u64,
}

impl State {
    pub fn new(
        downstream: StageRef<DecodedChainSyncEvent>,
        failures: StageRef<ValidationFailed>,
        errors: StageRef<ProcessingFailed>,
    ) -> Self {
        Self {
            downstream,
            failures,
            errors,
            recv_count: 0,
        }
    }
}

pub fn stage(
    mut state: State,
    msg: ChainSyncEvent,
    eff: impl ConsensusOps,
) -> impl Future<Output = State> {
    let span = tracing::trace_span!(parent: msg.span(), "chain_sync.receive_header");
    async move {
        match msg {
            ChainSyncEvent::RollForward {
                peer,
                raw_header,
                span,
                tip,
            } => {
                // TODO: check the point vs the deserialized header point and invalidate if they don't match
                // then simplify and don't pass the point separately
                let header = match decode_header(raw_header.as_slice()) {
                    Ok(header) => header,
                    Err(error) => {
                        tracing::error!(%error, %peer,
                            "chain_sync.receive_header.decode_failed");
                        eff.base()
                            .send(&state.failures, ValidationFailed::new(&peer, error))
                            .await;
                        return state;
                    }
                };

                state.recv_count += 1;
                if header.point() == tip.point() {
                    tracing::info!(%peer, point = %header.point(), "received header");
                } else if state.recv_count & 0xff == 0 {
                    tracing::debug!(%peer, point = %header.point(), tip_point = %tip.point(), recv_count = %state.recv_count, "received header (catching up)");
                } else {
                    tracing::trace!(%peer, point = %header.point(), tip_point = %tip.point(), recv_count = %state.recv_count, "received header (catching up)");
                }

                let result = eff.store().store_header(&header);
                if let Err(error) = result {
                    eff.base()
                        .send(&state.errors, ProcessingFailed::new(&peer, error.into()))
                        .await;
                    return state;
                };

                eff.base()
                    .send(
                        &state.downstream,
                        DecodedChainSyncEvent::RollForward { peer, header, span },
                    )
                    .await;
            }
            ChainSyncEvent::Rollback {
                peer,
                rollback_point,
                span,
                tip,
            } => {
                tracing::info!(%peer, point = ?rollback_point, ?tip, "received rollback");
                let msg = DecodedChainSyncEvent::Rollback {
                    peer,
                    rollback_point,
                    span,
                };
                eff.base().send(&state.downstream, msg).await
            }
        }
        state
    }
    .instrument(span)
}

#[instrument(
        level = Level::TRACE,
        skip_all,
        name = "chain_sync.decode_header",
)]
pub fn decode_header(raw_header: &[u8]) -> Result<BlockHeader, ConsensusError> {
    from_cbor_no_leftovers(raw_header).map_err(|reason| ConsensusError::CannotDecodeHeader {
        header: raw_header.into(),
        reason: reason.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consensus::{effects::mock_consensus_ops, errors::ValidationFailed};
    use amaru_kernel::{IsHeader, Peer, Point, Tip, any_header, cbor, utils::tests::run_strategy};
    use amaru_ouroboros::ChainStore;
    use pure_stage::StageRef;
    use std::collections::BTreeMap;
    use tracing::Span;

    #[tokio::test]
    async fn a_header_that_can_be_decoded_is_sent_downstream() -> anyhow::Result<()> {
        let peer = Peer::new("name");
        let header = run_strategy(any_header());
        let message = ChainSyncEvent::RollForward {
            peer: peer.clone(),
            tip: header.tip(),
            span: Span::current(),
            raw_header: cbor::to_vec(header.clone())?,
        };
        let consensus_ops = mock_consensus_ops();

        stage(make_state(), message, consensus_ops.clone()).await;

        let forwarded = DecodedChainSyncEvent::RollForward {
            peer: peer.clone(),
            header,
            span: Span::current(),
        };
        assert_eq!(
            consensus_ops.mock_base.received(),
            BTreeMap::from_iter(vec![(
                "downstream".to_string(),
                vec![format!("{forwarded:?}")]
            )])
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_header_that_cannot_be_decoded_sends_an_error() -> anyhow::Result<()> {
        let peer = Peer::new("name");
        let message = ChainSyncEvent::RollForward {
            peer: peer.clone(),
            tip: Tip::new(Point::Origin, 0.into()),
            span: Span::current(),
            raw_header: vec![1, 2, 3],
        };
        let consensus_ops = mock_consensus_ops();

        stage(make_state(), message, consensus_ops.clone()).await;

        let error = ValidationFailed::new(
            &peer,
            ConsensusError::CannotDecodeHeader {
                header: vec![1, 2, 3],
                reason: "unexpected type u8 at position 0: expected array".to_string(),
            },
        );
        assert_eq!(
            consensus_ops.mock_base.received(),
            BTreeMap::from_iter(vec![("failures".to_string(), vec![format!("{error:?}")])])
        );
        Ok(())
    }

    #[tokio::test]
    async fn rollback_is_just_sent_downstream() -> anyhow::Result<()> {
        let header = run_strategy(any_header());
        let peer = Peer::new("name");
        let span = Span::current();
        let message = ChainSyncEvent::Rollback {
            peer: peer.clone(),
            rollback_point: header.point(),
            tip: header.tip(),
            span: Span::current(),
        };
        let consensus_ops = mock_consensus_ops();
        consensus_ops.mock_store.store_header(&header)?;

        stage(make_state(), message, consensus_ops.clone()).await;

        let expected = DecodedChainSyncEvent::Rollback {
            peer: peer.clone(),
            rollback_point: header.point(),
            span,
        };
        assert_eq!(
            consensus_ops.mock_base.received(),
            BTreeMap::from_iter(vec![(
                "downstream".to_string(),
                vec![format!("{expected:?}")]
            )])
        );
        Ok(())
    }

    #[test]
    fn decode_header_on_generated_header() {
        let header = run_strategy(any_header());
        let raw_header = cbor::to_vec(header.clone()).unwrap();
        let decoded = decode_header(&raw_header).unwrap();
        assert_eq!(header, decoded);
    }

    // HELPERS

    fn make_state() -> State {
        let downstream: StageRef<DecodedChainSyncEvent> = StageRef::named_for_tests("downstream");
        let failures: StageRef<ValidationFailed> = StageRef::named_for_tests("failures");
        let errors: StageRef<ProcessingFailed> = StageRef::named_for_tests("errors");
        State::new(downstream, failures, errors)
    }
}
