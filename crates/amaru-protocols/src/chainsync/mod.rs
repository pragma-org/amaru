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

mod initiator;
mod messages;
mod responder;

pub use initiator::{ChainSyncInitiator, ChainSyncInitiatorMsg, InitiatorMessage, InitiatorResult, initiator};
pub use messages::HeaderContent;
pub use responder::{ChainSyncResponder, ResponderMessage, responder};

/// Number of RequestNext we keep in flight to not be limited by round-trip time.
/// This value has been obtained by testing between European countries and may therefore be too low for
/// catching up across continents; that might not be a smart use-case, though, which is why we use this
/// value for now.
///
/// Note that this also scales the buffer size limit accordingly.
pub const PIPELINE_DEPTH: u8 = 10;

pub fn register_deserializers() -> amaru_pure_stage::DeserializerGuards {
    vec![messages::register_deserializers(), initiator::register_deserializers(), responder::register_deserializers()]
        .into_iter()
        .flatten()
        .collect()
}

pub use register::{register_chainsync_initiator, register_chainsync_responder};

mod register {
    use amaru_kernel::{Peer, Point};
    use amaru_ouroboros::ConnectionId;
    use amaru_pure_stage::{Effects, StageRef};

    use super::*;
    use crate::{
        connection::ConnectionMessage,
        mux::{Frame, MuxMessage},
        protocol::{Inputs, PROTO_N2N_CHAIN_SYNC},
    };

    pub async fn register_chainsync_initiator(
        muxer: &StageRef<MuxMessage>,
        peer: Peer,
        conn_id: ConnectionId,
        pipeline: StageRef<ChainSyncInitiatorMsg>,
        eff: &Effects<ConnectionMessage>,
        tombstone: ConnectionMessage,
    ) -> StageRef<InitiatorMessage> {
        let chainsync = eff.stage("chainsync", initiator()).await;
        let chainsync = eff.supervise(chainsync, tombstone);
        let chainsync = eff.wire_up(chainsync, ChainSyncInitiator::new(peer, conn_id, muxer.clone(), pipeline)).await;
        eff.send(
            muxer,
            MuxMessage::Register {
                protocol: PROTO_N2N_CHAIN_SYNC.erase(),
                frame: Frame::OneCborItem,
                handler: chainsync.contramap(Inputs::Network),
                max_buffer: 5760 * usize::from(PIPELINE_DEPTH),
            },
        )
        .await;
        chainsync.contramap(Inputs::Local)
    }

    pub async fn register_chainsync_responder(
        muxer: &StageRef<MuxMessage>,
        upstream: Point,
        peer: Peer,
        conn_id: ConnectionId,
        eff: &Effects<ConnectionMessage>,
        tombstone: ConnectionMessage,
    ) -> StageRef<ResponderMessage> {
        let chainsync = eff.stage("chainsync-responder", responder()).await;
        let chainsync = eff.supervise(chainsync, tombstone);
        let chainsync = eff.wire_up(chainsync, ChainSyncResponder::new(upstream, peer, conn_id, muxer.clone())).await;
        eff.send(
            muxer,
            MuxMessage::Register {
                protocol: PROTO_N2N_CHAIN_SYNC.responder().erase(),
                frame: Frame::OneCborItem,
                handler: chainsync.contramap(Inputs::Network),
                max_buffer: 5760,
            },
        )
        .await;
        chainsync.contramap(Inputs::Local)
    }
}
