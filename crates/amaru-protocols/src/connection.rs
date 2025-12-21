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

use crate::keepalive::register_keepalive;
use crate::mux::HandlerMessage;
use crate::tx_submission::{TxSubmissionMessage, register_tx_submission};
use crate::{
    handshake::{self, HandshakeMessage, VersionTable},
    mux::{self, MuxMessage},
    protocol::{PROTO_HANDSHAKE, Role},
};
use amaru_kernel::peer::Peer;
use amaru_kernel::protocol_messages::{
    handshake::HandshakeResult, network_magic::NetworkMagic, version_data::VersionData,
    version_number::VersionNumber,
};
use amaru_ouroboros::{ConnectionId, TxOrigin};
use pure_stage::{Effects, StageRef};

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Connection {
    params: Params,
    state: State,
}

impl Connection {
    pub fn new(peer: Peer, conn_id: ConnectionId, role: Role, magic: NetworkMagic) -> Self {
        Self {
            params: Params {
                peer,
                conn_id,
                role,
                magic,
            },
            state: State::Initial,
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct Params {
    peer: Peer,
    conn_id: ConnectionId,
    role: Role,
    magic: NetworkMagic,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
enum State {
    Initial,
    Handshake {
        muxer: StageRef<MuxMessage>,
        handshake: StageRef<HandshakeMessage>,
    },
    Initiator {
        version_number: VersionNumber,
        version_data: VersionData,
        muxer: StageRef<MuxMessage>,
        handshake: StageRef<HandshakeMessage>,
        keepalive: StageRef<HandlerMessage>,
        tx_submission: StageRef<TxSubmissionMessage>,
    },
}

#[derive(Debug, PartialEq, Clone, serde::Serialize, serde::Deserialize)]
pub enum ConnectionMessage {
    Initialize,
    Handshake(HandshakeResult),
    // LATER: make full duplex, etc.
}

pub async fn stage(
    Connection { params, state }: Connection,
    msg: ConnectionMessage,
    eff: Effects<ConnectionMessage>,
) -> Connection {
    let state = match (state, msg) {
        (State::Initial, ConnectionMessage::Initialize) => do_initialize(&params, eff).await,
        (State::Handshake { muxer, handshake }, ConnectionMessage::Handshake(handshake_result)) => {
            do_handshake(&params, muxer, handshake, handshake_result, eff).await
        }
        x => unimplemented!("{x:?}"),
    };
    Connection { params, state }
}

async fn do_initialize(
    Params {
        conn_id,
        role,
        magic,
        ..
    }: &Params,
    eff: Effects<ConnectionMessage>,
) -> State {
    let muxer = eff.stage("mux", mux::stage).await;
    let muxer = eff
        .wire_up(
            muxer,
            mux::State::new(*conn_id, &[(PROTO_HANDSHAKE.erase(), 5760)]),
        )
        .await;

    let handshake_result = eff
        .contramap(eff.me(), "handshake_result", ConnectionMessage::Handshake)
        .await;

    let handshake = eff.stage("handshake", handshake::stage).await;
    let handshake = eff
        .wire_up(
            handshake,
            handshake::Handshake::new(
                muxer.clone(),
                handshake_result,
                *role,
                VersionTable::v11_and_above(*magic, true),
            ),
        )
        .await;

    let handler = eff
        .contramap(&handshake, "handshake_bytes", handshake::handler_transform)
        .await;

    eff.send(
        &muxer,
        MuxMessage::Register {
            protocol: PROTO_HANDSHAKE.erase(),
            frame: mux::Frame::OneCborItem,
            handler,
            max_buffer: 5760,
        },
    )
    .await;

    State::Handshake { muxer, handshake }
}

async fn do_handshake(
    Params { role, peer, .. }: &Params,
    muxer: StageRef<MuxMessage>,
    handshake: StageRef<HandshakeMessage>,
    handshake_result: HandshakeResult,
    eff: Effects<ConnectionMessage>,
) -> State {
    let (version_number, version_data) = match handshake_result {
        HandshakeResult::Accepted(version_number, version_data) => (version_number, version_data),
        HandshakeResult::Refused(refuse_reason) => {
            tracing::error!(?refuse_reason, "handshake refused");
            return eff.terminate().await;
        }
    };

    let keepalive = register_keepalive(*role, muxer.clone(), &eff).await;
    let tx_submission =
        register_tx_submission(*role, muxer.clone(), &eff, TxOrigin::Remote(peer.clone())).await;

    State::Initiator {
        version_number,
        version_data,
        muxer,
        handshake,
        keepalive,
        tx_submission,
    }
}
