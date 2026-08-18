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
#[cfg(test)]
mod tests;

use amaru_kernel::Peer;
use amaru_ouroboros::ConnectionId;
use amaru_pure_stage::{Effects, StageRef, Void};
pub use messages::{Cookie, Message};

use crate::{
    connection::ConnectionMessage,
    mux,
    protocol::{Inputs, PROTO_N2N_KEEP_ALIVE, ProtocolState},
};

pub fn register_deserializers() -> amaru_pure_stage::DeserializerGuards {
    vec![initiator::register_deserializers(), responder::register_deserializers()].into_iter().flatten().collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub enum State {
    Idle,
    Waiting,
}

pub fn spec<R: crate::protocol::RoleT>() -> crate::protocol::ProtoSpec<State, Message, R>
where
    State: ProtocolState<R, WireMsg = Message>,
{
    let mut spec = crate::protocol::ProtoSpec::default();
    let keep_alive = || Message::KeepAlive(Cookie::new());
    let response_keep_alive = || Message::ResponseKeepAlive(Cookie::new());

    // Initiator sends KeepAlive from Idle, transitions to Waiting
    spec.init(State::Idle, keep_alive(), State::Waiting);
    // Initiator receives ResponseKeepAlive in Waiting, transitions to Idle
    spec.resp(State::Waiting, response_keep_alive(), State::Idle);

    spec
}

pub async fn register_keepalive(
    role: crate::protocol::Role,
    peer: Peer,
    conn_id: ConnectionId,
    muxer: StageRef<crate::mux::MuxMessage>,
    eff: &Effects<ConnectionMessage>,
    tombstone: ConnectionMessage,
) -> StageRef<crate::mux::HandlerMessage> {
    let keepalive = if role == crate::protocol::Role::Initiator {
        let (state, stage) = initiator::KeepAliveInitiator::new(peer, conn_id, muxer.clone());
        let keepalive = eff.stage("keepalive", initiator::initiator()).await;
        let keepalive = eff.supervise(keepalive, tombstone);
        let keepalive = eff.wire_up(keepalive, (state, stage)).await;
        keepalive.contramap(Inputs::<initiator::InitiatorMessage>::Network)
    } else {
        let (state, stage) = responder::KeepAliveResponder::new(muxer.clone());
        let keepalive = eff.stage("keepalive", responder::responder()).await;
        let keepalive = eff.supervise(keepalive, tombstone);
        let keepalive = eff.wire_up(keepalive, (state, stage)).await;
        keepalive.contramap(Inputs::<Void>::Network)
    };

    eff.send(
        &muxer,
        crate::mux::MuxMessage::Register {
            protocol: PROTO_N2N_KEEP_ALIVE.for_role(role).erase(),
            frame: mux::Frame::OneCborItem,
            handler: keepalive.clone(),
            max_buffer: 65535,
        },
    )
    .await;

    keepalive
}
