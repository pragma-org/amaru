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
use crate::consensus::errors::{ConsensusError, ValidationFailed};
use crate::consensus::events::{ChainSyncEvent, DecodedChainSyncEvent};
use crate::consensus::span::adopt_current_span;
use amaru_kernel::{Hash, Header, MintedHeader, Point, cbor};
use pure_stage::StageRef;
use tracing::{Level, instrument};

type State = (StageRef<DecodedChainSyncEvent>, StageRef<ValidationFailed>);

#[instrument(
    level = Level::TRACE,
    skip_all,
    name = "stage.receive_header"
)]
pub async fn stage(
    (downstream, errors): State,
    msg: ChainSyncEvent,
    eff: impl ConsensusOps,
) -> State {
    adopt_current_span(&msg);
    match msg {
        ChainSyncEvent::RollForward {
            peer,
            point,
            raw_header,
            span,
        } => {
            let header = match decode_header(&point, raw_header.as_slice()) {
                Ok(header) => header,
                Err(error) => {
                    tracing::error!(%error, %point, %peer, "Failed to decode header");
                    eff.base()
                        .send(&errors, ValidationFailed::new(&peer, error))
                        .await;
                    return (downstream, errors);
                }
            };
            eff.base()
                .send(
                    &downstream,
                    DecodedChainSyncEvent::RollForward {
                        peer,
                        point,
                        header,
                        span,
                    },
                )
                .await;
        }
        ChainSyncEvent::Rollback {
            peer,
            rollback_point,
            span,
        } => {
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
        ChainSyncEvent::CaughtUp { peer, span } => {
            eff.base()
                .send(&downstream, DecodedChainSyncEvent::CaughtUp { peer, span })
                .await
        }
    }
    (downstream, errors)
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
pub fn decode_header(point: &Point, raw_header: &[u8]) -> Result<Header, ConsensusError> {
    let header: MintedHeader<'_> =
        cbor::decode(raw_header).map_err(|reason| ConsensusError::CannotDecodeHeader {
            point: point.clone(),
            header: raw_header.into(),
            reason: reason.to_string(),
        })?;

    Ok(Header::from(header))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consensus::effects::mock_consensus_ops;
    use crate::consensus::errors::ValidationFailed;
    use crate::consensus::events::DecodedChainSyncEvent;
    use crate::consensus::tests::any_header;
    use amaru_kernel::peer::Peer;
    use amaru_ouroboros_traits::fake::tests::run;
    use pure_stage::StageRef;
    use std::collections::BTreeMap;
    use tracing::Span;

    #[tokio::test]
    async fn a_header_that_can_be_decoded_is_sent_downstream() -> anyhow::Result<()> {
        let peer = Peer::new("name");
        let header = run(any_header());
        let message = ChainSyncEvent::RollForward {
            peer: peer.clone(),
            point: Point::Origin,
            span: Span::current(),
            raw_header: cbor::to_vec(header.clone())?,
        };
        let consensus_ops = mock_consensus_ops();

        stage(make_state(), message, consensus_ops.clone()).await;

        let forwarded = DecodedChainSyncEvent::RollForward {
            peer: peer.clone(),
            point: Point::Origin,
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
            BTreeMap::from_iter(vec![("errors".to_string(), vec![format!("{error:?}")])])
        );
        Ok(())
    }

    #[tokio::test]
    async fn rollback_is_just_sent_downstream() -> anyhow::Result<()> {
        let message = ChainSyncEvent::Rollback {
            peer: Peer::new("name"),
            rollback_point: Point::Origin,
            span: Span::current(),
        };
        let consensus_ops = mock_consensus_ops();

        stage(make_state(), message.clone(), consensus_ops.clone()).await;
        assert_eq!(
            consensus_ops.mock_base.received(),
            BTreeMap::from_iter(vec![(
                "downstream".to_string(),
                vec![format!("{message:?}")]
            )])
        );
        Ok(())
    }

    #[tokio::test]
    async fn caughtup_is_just_sent_downstream() -> anyhow::Result<()> {
        let message = ChainSyncEvent::CaughtUp {
            peer: Peer::new("name"),
            span: Span::current(),
        };
        let consensus_ops = mock_consensus_ops();

        stage(make_state(), message.clone(), consensus_ops.clone()).await;
        assert_eq!(
            consensus_ops.mock_base.received(),
            BTreeMap::from_iter(vec![(
                "downstream".to_string(),
                vec![format!("{message:?}")]
            )])
        );
        Ok(())
    }

    // HELPERS

    fn make_state() -> State {
        let downstream: StageRef<DecodedChainSyncEvent> = StageRef::named("downstream");
        let errors: StageRef<ValidationFailed> = StageRef::named("errors");
        (downstream, errors)
    }
}
