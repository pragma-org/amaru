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

use crate::consensus::effects::{BaseOps, ConsensusOps};
use crate::consensus::errors::{ConsensusError, ProcessingFailed, ValidationFailed};
use crate::consensus::span::HasSpan;
use amaru_kernel::consensus_events::{ChainSyncEvent, DecodedChainSyncEvent};
use amaru_kernel::{BlockHeader, Hash, Header, IsHeader, MintedHeader, Point, cbor};
use pure_stage::StageRef;
use tracing::{Level, instrument, span};

type State = (
    StageRef<DecodedChainSyncEvent>,
    StageRef<ChainSyncEvent>,
    StageRef<ValidationFailed>,
    StageRef<ProcessingFailed>,
);

pub async fn stage(
    (downstream, track_peers, failures, errors): State,
    msg: ChainSyncEvent,
    eff: impl ConsensusOps,
) -> State {
    let span = span!(parent: msg.span(), Level::TRACE, "stage.receive_header");
    let _entered = span.enter();

    match msg {
        ChainSyncEvent::RollForward {
            peer,
            point,
            raw_header,
            span,
        } => {
            // TODO: check the point vs the deserialized header point and invalidate if they don't match
            // then simplify and don't pass the point separately
            let header = match decode_header(&point, raw_header.as_slice()) {
                Ok(header) => header,
                Err(error) => {
                    tracing::error!(%error, %point, %peer, "Failed to decode header");
                    eff.base()
                        .send(&failures, ValidationFailed::new(&peer, error))
                        .await;
                    return (downstream, track_peers, failures, errors);
                }
            };

            if header.point() != point {
                tracing::error!(%point, %peer, "Header point {} does not match expected point {point}", header.point());
                let msg = ValidationFailed::new(
                    &peer,
                    ConsensusError::HeaderPointMismatch {
                        actual_point: header.point(),
                        expected_point: point.clone(),
                    },
                );
                eff.base().send(&failures, msg).await;
                return (downstream, track_peers, failures, errors);
            } else {
                let result = eff.store().store_header(&header);
                if let Err(error) = result {
                    eff.base()
                        .send(&errors, ProcessingFailed::new(&peer, error.into()))
                        .await;
                    return (downstream, track_peers, failures, errors);
                };
            }

            eff.base()
                .send(
                    &downstream,
                    DecodedChainSyncEvent::RollForward { peer, header, span },
                )
                .await;
        }
        ChainSyncEvent::Rollback {
            peer,
            rollback_point,
            span,
        } => {
            let msg = DecodedChainSyncEvent::Rollback {
                peer,
                rollback_point,
                span,
            };
            eff.base().send(&downstream, msg).await
        }
        ChainSyncEvent::CaughtUp { peer, span } => {
            eff.base()
                .send(&track_peers, ChainSyncEvent::CaughtUp { peer, span })
                .await
        }
    }
    (downstream, track_peers, failures, errors)
}

#[instrument(
        level = Level::TRACE,
        skip_all,
        name = "consensus.decode_header",
        fields(
            point.slot = %point.slot_or_default(),
            point.hash = %Hash::<32>::from(point),
        )
)]
pub fn decode_header(point: &Point, raw_header: &[u8]) -> Result<BlockHeader, ConsensusError> {
    let minted_header: MintedHeader<'_> =
        cbor::decode(raw_header).map_err(|reason| ConsensusError::CannotDecodeHeader {
            point: point.clone(),
            header: raw_header.into(),
            reason: reason.to_string(),
        })?;

    let header = Header::from(minted_header);
    Ok(BlockHeader::from(header))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consensus::effects::mock_consensus_ops;
    use crate::consensus::errors::ValidationFailed;
    use amaru_kernel::is_header::tests::{any_header, run};
    use amaru_kernel::peer::Peer;
    use amaru_ouroboros::ChainStore;
    use pure_stage::StageRef;
    use std::collections::BTreeMap;
    use tracing::Span;

    #[tokio::test]
    async fn a_header_that_can_be_decoded_is_sent_downstream() -> anyhow::Result<()> {
        let peer = Peer::new("name");
        let header = run(any_header());
        let message = ChainSyncEvent::RollForward {
            peer: peer.clone(),
            point: header.point(),
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
            point: Point::Origin,
            span: Span::current(),
            raw_header: vec![1, 2, 3],
        };
        let consensus_ops = mock_consensus_ops();

        stage(make_state(), message.clone(), consensus_ops.clone()).await;

        let error = ValidationFailed::new(
            &peer,
            ConsensusError::CannotDecodeHeader {
                point: Point::Origin,
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
    async fn a_header_point_that_does_not_match_the_expected_point_sends_an_error()
    -> anyhow::Result<()> {
        let peer = Peer::new("name");
        let header = run(any_header());
        let point = run(any_header()).point();
        let message = ChainSyncEvent::RollForward {
            peer: peer.clone(),
            point: point.clone(),
            span: Span::current(),
            raw_header: cbor::to_vec(header.clone())?,
        };
        let consensus_ops = mock_consensus_ops();

        stage(make_state(), message.clone(), consensus_ops.clone()).await;

        let error = ValidationFailed::new(
            &peer,
            ConsensusError::HeaderPointMismatch {
                actual_point: header.point(),
                expected_point: point.clone(),
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
        let header = run(any_header());
        let peer = Peer::new("name");
        let span = Span::current();
        let message = ChainSyncEvent::Rollback {
            peer: peer.clone(),
            rollback_point: header.point(),
            span: span.clone(),
        };
        let consensus_ops = mock_consensus_ops();
        consensus_ops.mock_store.store_header(&header)?;

        stage(make_state(), message.clone(), consensus_ops.clone()).await;

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

    #[tokio::test]
    async fn caughtup_is_sent_to_track_peers() -> anyhow::Result<()> {
        let message = ChainSyncEvent::CaughtUp {
            peer: Peer::new("name"),
            span: Span::current(),
        };
        let consensus_ops = mock_consensus_ops();

        stage(make_state(), message.clone(), consensus_ops.clone()).await;
        assert_eq!(
            consensus_ops.mock_base.received(),
            BTreeMap::from_iter(vec![(
                "track_peers".to_string(),
                vec![format!("{message:?}")]
            )])
        );
        Ok(())
    }

    #[test]
    fn decode_header_on_generated_header() {
        let header = run(any_header());
        let raw_header = cbor::to_vec(header.clone()).unwrap();
        let point = header.point();
        let decoded = decode_header(&point, &raw_header).unwrap();
        assert_eq!(header, decoded);
    }

    // HELPERS

    fn make_state() -> State {
        let downstream: StageRef<DecodedChainSyncEvent> = StageRef::named("downstream");
        let track_peers: StageRef<ChainSyncEvent> = StageRef::named("track_peers");
        let failures: StageRef<ValidationFailed> = StageRef::named("failures");
        let errors: StageRef<ProcessingFailed> = StageRef::named("errors");
        (downstream, track_peers, failures, errors)
    }
}
