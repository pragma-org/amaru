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

use crate::ConsensusError;
use amaru_kernel::{Hash, Header, MintedHeader, Point};
use pallas_codec::minicbor;
use tracing::{instrument, Level};

use super::{ChainSyncEvent, DecodedChainSyncEvent};

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
    let minted_header: MintedHeader<'_> =
        minicbor::decode(raw_header).map_err(|_| ConsensusError::CannotDecodeHeader {
            point: point.clone(),
            header: raw_header.into(),
        })?;

    Ok(Header::from(minted_header))
}

pub fn handle_chain_sync(
    chain_sync: ChainSyncEvent,
) -> Result<DecodedChainSyncEvent, ConsensusError> {
    match chain_sync {
        ChainSyncEvent::RollForward {
            peer,
            point,
            raw_header,
            span,
        } => {
            let header = receive_header(&point, &raw_header)?;
            Ok(DecodedChainSyncEvent::RollForward {
                peer,
                point,
                header,
                span,
            })
        }
        ChainSyncEvent::Rollback {
            peer,
            rollback_point,
            span,
        } => Ok(DecodedChainSyncEvent::Rollback {
            peer,
            rollback_point,
            span,
        }),
    }
}
