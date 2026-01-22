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
pub(crate) mod messages;
mod responder;

use crate::mux::{Frame, MuxMessage};
use crate::protocol::{Inputs, ProtoSpec, ProtocolState, RoleT};
use amaru_kernel::{Point, peer::Peer};
use amaru_ouroboros::ConnectionId;
use pure_stage::{DeserializerGuards, Effects, StageRef, Void};

// Re-export types
pub use initiator::{BlockFetchInitiator, BlockFetchMessage, Blocks, initiator};
pub use messages::Message;
pub use responder::{BlockFetchResponder, responder};

pub fn spec<R: RoleT>() -> ProtoSpec<State, Message, R>
where
    State: ProtocolState<R, WireMsg = Message>,
{
    use State::*;

    let mut spec = ProtoSpec::default();
    let request_range = || Message::RequestRange {
        from: Point::Origin,
        through: Point::Origin,
    };
    let no_blocks = || Message::NoBlocks;
    let client_done = || Message::ClientDone;
    let batch_done = || Message::BatchDone;
    let start_batch = || Message::StartBatch;
    let block = || Message::Block { body: vec![] };

    spec.init(Idle, client_done(), Done);
    spec.init(Idle, request_range(), Busy);
    spec.resp(Busy, no_blocks(), Idle);
    spec.resp(Busy, start_batch(), Streaming);
    spec.resp(Streaming, block(), Streaming);
    spec.resp(Streaming, batch_done(), Idle);
    spec
}

pub fn register_deserializers() -> DeserializerGuards {
    vec![
        initiator::register_deserializers(),
        responder::register_deserializers(),
    ]
    .into_iter()
    .flatten()
    .collect()
}

#[derive(
    Debug, PartialEq, Eq, Clone, Copy, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub enum State {
    Idle,
    Busy,
    Streaming,
    Done,
}

pub async fn register_blockfetch_initiator<M>(
    muxer: &StageRef<MuxMessage>,
    peer: Peer,
    conn_id: ConnectionId,
    eff: &Effects<M>,
) -> StageRef<BlockFetchMessage> {
    use crate::protocol::PROTO_N2N_BLOCK_FETCH;
    let blockfetch = eff
        .wire_up(
            eff.stage("blockfetch", initiator()).await,
            BlockFetchInitiator::new(muxer.clone(), peer, conn_id),
        )
        .await;
    eff.send(
        muxer,
        MuxMessage::Register {
            protocol: PROTO_N2N_BLOCK_FETCH.erase(),
            frame: Frame::OneCborItem,
            handler: eff
                .contramap(&blockfetch, "blockfetch_bytes", Inputs::Network)
                .await,
            max_buffer: 25000000,
        },
    )
    .await;
    eff.contramap(&blockfetch, "blockfetch_bytes", Inputs::Local)
        .await
}

pub async fn register_blockfetch_responder<M>(
    muxer: &StageRef<MuxMessage>,
    eff: &Effects<M>,
) -> StageRef<Void> {
    use crate::protocol::PROTO_N2N_BLOCK_FETCH;
    let blockfetch = eff
        .wire_up(
            eff.stage("blockfetch", responder()).await,
            BlockFetchResponder::new(muxer.clone()),
        )
        .await;
    eff.send(
        muxer,
        MuxMessage::Register {
            protocol: PROTO_N2N_BLOCK_FETCH.responder().erase(),
            frame: Frame::OneCborItem,
            handler: eff
                .contramap(&blockfetch, "blockfetch_bytes", Inputs::Network)
                .await,
            max_buffer: 25000000,
        },
    )
    .await;
    eff.contramap(&blockfetch, "blockfetch_handler", Inputs::Local)
        .await
}
