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

use crate::{ConsensusError, span::adopt_current_span};
use amaru_kernel::{Hash, Header, MintedHeader, Point, cbor};
use tracing::{Level, instrument};

use super::{ChainSyncEvent, DecodedChainSyncEvent, ValidationFailed};
use pure_stage::{Effects, StageRef, Void};

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

type State = (
    StageRef<DecodedChainSyncEvent, Void>,
    StageRef<ValidationFailed, Void>,
);

#[instrument(
    level = Level::TRACE,
    skip_all,
    name = "stage.receive_header"
)]
pub async fn stage(
    (downstream, errors): State,
    msg: ChainSyncEvent,
    eff: Effects<ChainSyncEvent, State>,
) -> State {
    let span = adopt_current_span(&msg);
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
                    eff.send(&errors, ValidationFailed::new(peer, point.clone(), error))
                        .await;
                    return (downstream, errors);
                }
            };
            eff.send(
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
            eff.send(
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
            eff.send(
                &downstream,
                DecodedChainSyncEvent::CaughtUp {
                    peer,
                    span,
                },
            )
            .await
        }
    }
    (downstream, errors)
}
