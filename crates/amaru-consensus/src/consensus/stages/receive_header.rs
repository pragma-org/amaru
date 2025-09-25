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

#[instrument(
        level = Level::TRACE,
        skip_all,
        name = "consensus.receive_header",
        fields(
            point.slot = %point.slot_or_default(),
            point.hash = %Hash::<32>::from(point),
        )
)]
pub fn receive_header(point: &Point, raw_header: &[u8]) -> Result<Header, ConsensusError> {
    let header: MintedHeader<'_> =
        cbor::decode(raw_header).map_err(|reason| ConsensusError::CannotDecodeHeader {
            point: point.clone(),
            header: raw_header.into(),
            reason: reason.to_string(),
        })?;

    Ok(Header::from(header))
}

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
            let header = match receive_header(&point, raw_header.as_slice()) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consensus::effects::mock_consensus_ops;
    use crate::consensus::errors::ValidationFailed;
    use crate::consensus::events::DecodedChainSyncEvent;
    use amaru_kernel::peer::Peer;
    use pure_stage::StageRef;
    use std::collections::BTreeMap;
    use tracing::Span;

    #[tokio::test]
    async fn rollback_is_just_sent_downstream() -> anyhow::Result<()> {
        let message = ChainSyncEvent::Rollback {
            peer: Peer::new("name"),
            rollback_point: Point::Origin,
            span: Span::current(),
        };
        let mut consensus_ops = mock_consensus_ops();

        stage(make_state(), message.clone(), &mut consensus_ops).await;
        assert_eq!(
            consensus_ops.base().received(),
            BTreeMap::from_iter(vec![("downstream".to_string(), format!("{message:?}"))])
        );
        Ok(())
    }

    #[tokio::test]
    async fn caughtup_is_just_sent_downstream() -> anyhow::Result<()> {
        let message = ChainSyncEvent::CaughtUp {
            peer: Peer::new("name"),
            span: Span::current(),
        };
        let mut consensus_ops = mock_consensus_ops();

        stage(make_state(), message.clone(), &mut consensus_ops).await;
        assert_eq!(
            consensus_ops.base().received(),
            BTreeMap::from_iter(vec![("downstream".to_string(), format!("{message:?}"))])
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
