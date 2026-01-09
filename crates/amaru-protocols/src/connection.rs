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

use crate::blockfetch::{
    self, BlockFetchMessage, Blocks, register_blockfetch_initiator, register_blockfetch_responder,
};
use crate::chainsync::{
    self, ChainSyncInitiatorMsg, register_chainsync_initiator, register_chainsync_responder,
};
use crate::keepalive::register_keepalive;
use crate::mux::HandlerMessage;
use crate::protocol::Inputs;
use crate::store_effects::Store;
use crate::tx_submission::register_tx_submission;
use crate::{
    handshake,
    mux::{self, MuxMessage},
    protocol::{PROTO_HANDSHAKE, Role},
};
use amaru_kernel::Point;
use amaru_kernel::peer::Peer;
use amaru_kernel::protocol_messages::version_table::VersionTable;
use amaru_kernel::protocol_messages::{
    handshake::HandshakeResult, network_magic::NetworkMagic, version_data::VersionData,
    version_number::VersionNumber,
};
use amaru_ouroboros::{ConnectionId, ReadOnlyChainStore, TxOrigin};
use pure_stage::{Effects, StageRef, Void};

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Connection {
    params: Params,
    state: State,
}

impl Connection {
    pub fn new(
        peer: Peer,
        conn_id: ConnectionId,
        role: Role,
        magic: NetworkMagic,
        pipeline: StageRef<ChainSyncInitiatorMsg>,
    ) -> Self {
        Self {
            params: Params {
                peer,
                conn_id,
                role,
                magic,
                pipeline,
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
    pipeline: StageRef<ChainSyncInitiatorMsg>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
enum State {
    Initial,
    Handshake {
        muxer: StageRef<MuxMessage>,
        handshake: StageRef<Inputs<Void>>,
    },
    Initiator(StateInitiator),
    Responder(StateResponder),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct StateInitiator {
    chainsync_initiator: StageRef<chainsync::InitiatorMessage>,
    blockfetch_initiator: StageRef<blockfetch::BlockFetchMessage>,
    version_number: VersionNumber,
    version_data: VersionData,
    muxer: StageRef<MuxMessage>,
    handshake: StageRef<Inputs<Void>>,
    keepalive: StageRef<HandlerMessage>,
    tx_submission: StageRef<HandlerMessage>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct StateResponder {
    chainsync_responder: StageRef<chainsync::ResponderMessage>,
    muxer: StageRef<MuxMessage>,
    handshake: StageRef<Inputs<Void>>,
    keepalive: StageRef<HandlerMessage>,
    tx_submission: StageRef<HandlerMessage>,
    blockfetch_responder: StageRef<Void>,
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ConnectionMessage {
    Initialize,
    Disconnect,
    Handshake(HandshakeResult),
    FetchBlocks {
        from: Point,
        through: Point,
        cr: StageRef<Blocks>,
    },
    // LATER: make full duplex, etc.
}

pub async fn stage(
    Connection { params, state }: Connection,
    msg: ConnectionMessage,
    eff: Effects<ConnectionMessage>,
) -> Connection {
    let state = match (state, msg) {
        (_, ConnectionMessage::Disconnect) => return eff.terminate().await,
        (State::Initial, ConnectionMessage::Initialize) => do_initialize(&params, eff).await,
        (State::Handshake { muxer, handshake }, ConnectionMessage::Handshake(handshake_result)) => {
            do_handshake(
                &params,
                muxer,
                params.pipeline.clone(),
                handshake,
                handshake_result,
                eff,
            )
            .await
        }
        (State::Initiator(s), ConnectionMessage::FetchBlocks { from, through, cr }) => {
            eff.send(
                &s.blockfetch_initiator,
                BlockFetchMessage::RequestRange { from, through, cr },
            )
            .await;
            State::Initiator(s)
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

    let handshake = match role {
        Role::Initiator => {
            eff.wire_up(
                eff.stage("handshake", handshake::initiator()).await,
                handshake::HandshakeInitiator::new(
                    muxer.clone(),
                    handshake_result,
                    VersionTable::v11_and_above(*magic, true),
                ),
            )
            .await
        }
        Role::Responder => {
            eff.wire_up(
                eff.stage("handshake", handshake::responder()).await,
                handshake::HandshakeResponder::new(
                    muxer.clone(),
                    handshake_result,
                    VersionTable::v11_and_above(*magic, true),
                ),
            )
            .await
        }
    };

    let handler = eff
        .contramap(&handshake, "handshake_bytes", Inputs::Network)
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

#[expect(clippy::expect_used)]
async fn do_handshake(
    Params {
        role,
        peer,
        conn_id,
        ..
    }: &Params,
    muxer: StageRef<MuxMessage>,
    pipeline: StageRef<ChainSyncInitiatorMsg>,
    handshake: StageRef<Inputs<Void>>,
    handshake_result: HandshakeResult,
    eff: Effects<ConnectionMessage>,
) -> State {
    let (version_number, version_data) = match handshake_result {
        HandshakeResult::Accepted(version_number, version_data) => (version_number, version_data),
        HandshakeResult::Refused(refuse_reason) => {
            tracing::error!(?refuse_reason, "handshake refused");
            return eff.terminate().await;
        }
        HandshakeResult::Query(version_table) => {
            tracing::info!(?version_table, "handshake query reply");
            return eff.terminate().await;
        }
    };

    let keepalive = register_keepalive(*role, muxer.clone(), &eff).await;
    let tx_submission =
        register_tx_submission(*role, muxer.clone(), &eff, TxOrigin::Remote(peer.clone())).await;

    if *role == Role::Initiator {
        let chainsync_initiator =
            register_chainsync_initiator(&muxer, peer.clone(), *conn_id, pipeline, &eff).await;
        let blockfetch_initiator =
            register_blockfetch_initiator(&muxer, peer.clone(), *conn_id, &eff).await;
        State::Initiator(StateInitiator {
            chainsync_initiator,
            blockfetch_initiator,
            version_number,
            version_data,
            muxer,
            handshake,
            keepalive,
            tx_submission,
        })
    } else {
        let store = Store::new(eff.clone());
        let upstream = store.get_best_chain_hash();
        let header = store
            .load_header(&upstream)
            .expect("best chain hash not found");
        let upstream = header.tip();
        let chainsync_responder =
            register_chainsync_responder(&muxer, upstream, peer.clone(), *conn_id, &eff).await;
        let blockfetch_responder = register_blockfetch_responder(&muxer, &eff).await;

        State::Responder(StateResponder {
            chainsync_responder,
            blockfetch_responder,
            muxer,
            handshake,
            keepalive,
            tx_submission,
        })
    }
}
