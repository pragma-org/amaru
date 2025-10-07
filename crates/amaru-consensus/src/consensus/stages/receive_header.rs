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
use crate::consensus::events::{ChainSyncEvent, NewHeader, ValidateBlock};
use crate::consensus::span::adopt_current_span;
use amaru_kernel::{Hash, Header, MintedHeader, Point, cbor};
use pure_stage::StageRef;
use tracing::{Level, instrument};

type State = (
    StageRef<NewHeader>,
    StageRef<ValidateBlock>,
    StageRef<ChainSyncEvent<Header>>,
    StageRef<ValidationFailed>,
);

#[instrument(
    level = Level::TRACE,
    skip_all,
    name = "stage.receive_header"
)]
pub async fn stage(
    (store_header, validate_block, select_chain, errors): State,
    msg: ChainSyncEvent<Vec<u8>>,
    eff: impl ConsensusOps,
) -> State {
    adopt_current_span(&msg);
    match msg {
        ChainSyncEvent::RollForward {
            peer, point, value, ..
        } => {
            let header = match decode_header(&point, value.as_slice()) {
                Ok(header) => header,
                Err(error) => {
                    tracing::error!(%error, %point, %peer, "Failed to decode header");
                    eff.base()
                        .send(&errors, ValidationFailed::new(&peer, error))
                        .await;
                    return (store_header, validate_block, select_chain, errors);
                }
            };
            eff.base()
                .send(&store_header, NewHeader::new(peer, point, header))
                .await;
        }
        ChainSyncEvent::Rollback { peer, point, span } => {
            eff.base()
                .send(
                    &validate_block,
                    ValidateBlock::Rollback { peer, point, span },
                )
                .await
        }
        ChainSyncEvent::CaughtUp { peer, span } => {
            eff.base()
                .send(&select_chain, ChainSyncEvent::CaughtUp { peer, span })
                .await
        }
    }
    (store_header, validate_block, select_chain, errors)
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
    use crate::consensus::events::{ChainSyncEvent, NewHeader};
    use crate::consensus::tests::any_header;
    use amaru_kernel::peer::Peer;
    use amaru_ouroboros_traits::fake::tests::run;
    use pure_stage::StageRef;
    use std::collections::BTreeMap;
    use tracing::Span;

    #[tokio::test]
    async fn a_header_that_can_be_decoded_is_sent_to_store_header() -> anyhow::Result<()> {
        let peer = Peer::new("name");
        let header = run(any_header());
        let message = ChainSyncEvent::RollForward {
            peer: peer.clone(),
            point: Point::Origin,
            span: Span::current(),
            value: cbor::to_vec(header.clone())?,
        };
        let consensus_ops = mock_consensus_ops();

        stage(make_state(), message, consensus_ops.clone()).await;

        let forwarded = NewHeader::new(peer, Point::Origin, header);
        assert_eq!(
            consensus_ops.mock_base.received(),
            BTreeMap::from_iter(vec![(
                "store_header".to_string(),
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
            value: vec![1, 2, 3],
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
    async fn rollback_is_sent_to_validate_block() -> anyhow::Result<()> {
        let message = ChainSyncEvent::Rollback {
            peer: Peer::new("name"),
            point: Point::Origin,
            span: Span::current(),
        };
        let consensus_ops = mock_consensus_ops();

        stage(make_state(), message.clone(), consensus_ops.clone()).await;
        let message = ValidateBlock::Rollback {
            peer: Peer::new("name"),
            point: Point::Origin,
            span: Span::current(),
        };
        assert_eq!(
            consensus_ops.mock_base.received(),
            BTreeMap::from_iter(vec![(
                "validate_block".to_string(),
                vec![format!("{message:?}")]
            )])
        );
        Ok(())
    }

    #[tokio::test]
    async fn caught_up_is_just_sent_to_select_chain() -> anyhow::Result<()> {
        let message = ChainSyncEvent::CaughtUp {
            peer: Peer::new("name"),
            span: Span::current(),
        };
        let consensus_ops = mock_consensus_ops();

        stage(make_state(), message.clone(), consensus_ops.clone()).await;
        assert_eq!(
            consensus_ops.mock_base.received(),
            BTreeMap::from_iter(vec![(
                "select_chain".to_string(),
                vec![format!("{message:?}")]
            )])
        );
        Ok(())
    }

    // HELPERS

    fn make_state() -> State {
        let store_header: StageRef<NewHeader> = StageRef::named("store_header");
        let validate_block: StageRef<ValidateBlock> = StageRef::named("validate_block");
        let select_chain: StageRef<ChainSyncEvent<Header>> = StageRef::named("select_chain");
        let errors: StageRef<ValidationFailed> = StageRef::named("errors");
        (store_header, validate_block, select_chain, errors)
    }
}
