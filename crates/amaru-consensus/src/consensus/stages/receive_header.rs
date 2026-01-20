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
use amaru_kernel::{BlockHeader, Header, MintedHeader, cbor};
use amaru_observability::consensus::chain_sync::{
    DECODE_HEADER, RECEIVE_HEADER, RECEIVE_HEADER_DECODE_FAILED,
};
use pure_stage::StageRef;
use tracing::{Instrument, Level, instrument};

type State = (
    StageRef<DecodedChainSyncEvent>,
    StageRef<ValidationFailed>,
    StageRef<ProcessingFailed>,
);

pub fn stage(
    (downstream, failures, errors): State,
    msg: ChainSyncEvent,
    eff: impl ConsensusOps,
) -> impl Future<Output = State> {
    let span = tracing::trace_span!(parent: msg.span(), RECEIVE_HEADER);
    async move {
        match msg {
            ChainSyncEvent::RollForward {
                peer,
                raw_header,
                span,
                ..
            } => {
                // TODO: check the point vs the deserialized header point and invalidate if they don't match
                // then simplify and don't pass the point separately
                let header = match decode_header(raw_header.as_slice()) {
                    Ok(header) => header,
                    Err(error) => {
                        tracing::error!(%error, %peer,
                            RECEIVE_HEADER_DECODE_FAILED);
                        eff.base()
                            .send(&failures, ValidationFailed::new(&peer, error))
                            .await;
                        return (downstream, failures, errors);
                    }
                };

                let result = eff.store().store_header(&header);
                if let Err(error) = result {
                    eff.base()
                        .send(&errors, ProcessingFailed::new(&peer, error.into()))
                        .await;
                    return (downstream, failures, errors);
                };

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
                ..
            } => {
                let msg = DecodedChainSyncEvent::Rollback {
                    peer,
                    rollback_point,
                    span,
                };
                eff.base().send(&downstream, msg).await
            }
        }
        (downstream, failures, errors)
    }
    .instrument(span)
}

#[instrument(
        level = Level::TRACE,
        skip_all,
        name = DECODE_HEADER,
)]
pub fn decode_header(raw_header: &[u8]) -> Result<BlockHeader, ConsensusError> {
    let minted_header: MintedHeader<'_> =
        cbor::decode(raw_header).map_err(|reason| ConsensusError::CannotDecodeHeader {
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
    use amaru_kernel::protocol_messages::tip::Tip;
    use amaru_kernel::{IsHeader, Point};
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
        let header = run(any_header());
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
        let header = run(any_header());
        let raw_header = cbor::to_vec(header.clone()).unwrap();
        let decoded = decode_header(&raw_header).unwrap();
        assert_eq!(header, decoded);
    }

    // HELPERS

    fn make_state() -> State {
        let downstream: StageRef<DecodedChainSyncEvent> = StageRef::named_for_tests("downstream");
        let failures: StageRef<ValidationFailed> = StageRef::named_for_tests("failures");
        let errors: StageRef<ProcessingFailed> = StageRef::named_for_tests("errors");
        (downstream, failures, errors)
    }
}
