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

pub use initiator::{
    ChainSyncInitiator, ChainSyncInitiatorMsg, InitiatorMessage, InitiatorResult, initiator,
};
pub use responder::{ChainSyncResponder, ResponderMessage, responder};

pub fn register_deserializers() -> pure_stage::DeserializerGuards {
    vec![
        messages::register_deserializers(),
        initiator::register_deserializers(),
        responder::register_deserializers(),
    ]
    .into_iter()
    .flatten()
    .collect()
}

pub use register::{register_chainsync_initiator, register_chainsync_responder};

mod register {
    use super::*;
    use crate::{
        connection::ConnectionMessage,
        mux::{Frame, MuxMessage},
        protocol::{Inputs, PROTO_N2N_CHAIN_SYNC},
    };
    use amaru_kernel::{peer::Peer, protocol_messages::tip::Tip};
    use amaru_ouroboros::ConnectionId;
    use pure_stage::{Effects, StageRef};

    pub async fn register_chainsync_initiator(
        muxer: &StageRef<MuxMessage>,
        peer: Peer,
        conn_id: ConnectionId,
        pipeline: StageRef<ChainSyncInitiatorMsg>,
        eff: &Effects<ConnectionMessage>,
    ) -> StageRef<InitiatorMessage> {
        let chainsync = eff
            .wire_up(
                eff.stage("chainsync", initiator()).await,
                ChainSyncInitiator::new(peer, conn_id, muxer.clone(), pipeline),
            )
            .await;
        eff.send(
            muxer,
            MuxMessage::Register {
                protocol: PROTO_N2N_CHAIN_SYNC.erase(),
                frame: Frame::OneCborItem,
                handler: eff
                    .contramap(&chainsync, "chainsync_bytes", Inputs::Network)
                    .await,
                max_buffer: 5760,
            },
        )
        .await;
        eff.contramap(&chainsync, "chainsync_bytes", Inputs::Local)
            .await
    }

    pub async fn register_chainsync_responder(
        muxer: &StageRef<MuxMessage>,
        upstream: Tip,
        peer: Peer,
        conn_id: ConnectionId,
        eff: &Effects<ConnectionMessage>,
    ) -> StageRef<ResponderMessage> {
        let chainsync = eff
            .wire_up(
                eff.stage("chainsync", responder()).await,
                ChainSyncResponder::new(upstream, peer, conn_id, muxer.clone()),
            )
            .await;
        eff.send(
            muxer,
            MuxMessage::Register {
                protocol: PROTO_N2N_CHAIN_SYNC.responder().erase(),
                frame: Frame::OneCborItem,
                handler: eff
                    .contramap(&chainsync, "chainsync_bytes", Inputs::Network)
                    .await,
                max_buffer: 5760,
            },
        )
        .await;
        eff.contramap(&chainsync, "chainsync_bytes", Inputs::Local)
            .await
    }
}
