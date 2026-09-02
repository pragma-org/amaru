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
mod pipelined;
mod responder;

use amaru_kernel::{NetworkPoint, Peer};
use amaru_ouroboros::ConnectionId;
use amaru_pure_stage::{DeserializerGuards, Effects, StageRef};
// Re-export types
pub use initiator::{BlockFetchInitiator, BlockFetchMessage, Blocks, initiator};
pub use messages::{BatchDone, Block, ClientDone, Message, NoBlocks, RequestRange, StartBatch};
pub use pipelined::{BLOCKFETCH_PIPELINE_N, register_blockfetch_initiator_pipelined};
pub use responder::{BlockFetchResponder, StreamBlocks, responder};

use crate::{
    mux::{Frame, MuxMessage},
    protocol::{Inputs, ProtoSpec, ProtocolState, RoleT},
};

pub fn spec<R: RoleT>() -> ProtoSpec<State, Message, R>
where
    State: ProtocolState<R, WireMsg = Message>,
{
    use State::*;

    let mut spec = ProtoSpec::default();
    let request_range = || RequestRange { from: NetworkPoint::Origin, through: NetworkPoint::Origin }.into();
    let no_blocks = || NoBlocks.into();
    let client_done = || ClientDone.into();
    let batch_done = || BatchDone.into();
    let start_batch = || StartBatch.into();
    let block = || Block { body: vec![1] }.into();

    spec.init(Idle, request_range(), Busy);
    if R::ROLE == Some(crate::protocol::Role::Initiator) {
        spec.init(Idle, client_done(), Done);
    } else {
        spec.init(Idle, client_done(), Idle);
    }
    spec.resp(Busy, no_blocks(), Idle);
    spec.resp(Busy, start_batch(), Streaming);
    spec.resp(Streaming, block(), Streaming);
    spec.resp(Streaming, batch_done(), Idle);
    spec
}

pub fn register_deserializers() -> DeserializerGuards {
    vec![initiator::register_deserializers(), pipelined::register_deserializers(), responder::register_deserializers()]
        .into_iter()
        .flatten()
        .collect()
}

#[derive(Debug, PartialEq, Eq, Clone, Copy, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub enum State {
    Idle,
    Busy,
    Streaming,
    Done,
}

pub async fn register_blockfetch_initiator<M: amaru_pure_stage::SendData>(
    muxer: &StageRef<MuxMessage>,
    peer: Peer,
    conn_id: ConnectionId,
    eff: &Effects<M>,
    tombstone: M,
) -> StageRef<BlockFetchMessage> {
    use crate::protocol::PROTO_N2N_BLOCK_FETCH;
    let blockfetch = eff.stage("blockfetch", initiator()).await;
    let blockfetch = eff.supervise(blockfetch, tombstone);
    let blockfetch = eff.wire_up(blockfetch, BlockFetchInitiator::new(muxer.clone(), peer, conn_id)).await;
    eff.send(
        muxer,
        MuxMessage::Register {
            protocol: PROTO_N2N_BLOCK_FETCH.erase(),
            frame: Frame::OneCborItem,
            handler: blockfetch.contramap(Inputs::Network),
            max_buffer: 2_500_000,
        },
    )
    .await;
    blockfetch.contramap(Inputs::Local)
}

pub async fn register_blockfetch_responder<M: amaru_pure_stage::SendData>(
    muxer: &StageRef<MuxMessage>,
    eff: &Effects<M>,
    tombstone: M,
) -> StageRef<StreamBlocks> {
    use crate::protocol::PROTO_N2N_BLOCK_FETCH;
    let blockfetch = eff.stage("blockfetch", responder()).await;
    let blockfetch = eff.supervise(blockfetch, tombstone);
    let blockfetch = eff.wire_up(blockfetch, BlockFetchResponder::new(muxer.clone())).await;
    eff.send(
        muxer,
        MuxMessage::Register {
            protocol: PROTO_N2N_BLOCK_FETCH.responder().erase(),
            frame: Frame::OneCborItem,
            handler: blockfetch.contramap(Inputs::Network),
            max_buffer: 2_500_000,
        },
    )
    .await;
    blockfetch.contramap(Inputs::Local)
}
