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

use crate::{
    connection::ConnectionMessage,
    mux,
    protocol::{Inputs, PROTO_N2N_KEEP_ALIVE, ProtocolState},
};
use pure_stage::{Effects, StageRef, Void};

pub use messages::{Cookie, Message};

pub fn register_deserializers() -> pure_stage::DeserializerGuards {
    vec![
        initiator::register_deserializers(),
        responder::register_deserializers(),
    ]
    .into_iter()
    .flatten()
    .collect()
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
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
    muxer: StageRef<crate::mux::MuxMessage>,
    eff: &Effects<ConnectionMessage>,
) -> StageRef<crate::mux::HandlerMessage> {
    let keepalive = if role == crate::protocol::Role::Initiator {
        let (state, stage) = initiator::KeepAliveInitiator::new(muxer.clone());
        let keepalive = eff
            .wire_up(
                eff.stage("keepalive", initiator::initiator()).await,
                (state, stage),
            )
            .await;
        eff.contramap(
            &keepalive,
            "keepalive_handler",
            Inputs::<initiator::InitiatorMessage>::Network,
        )
        .await
    } else {
        let (state, stage) = responder::KeepAliveResponder::new(muxer.clone());
        let keepalive = eff
            .wire_up(
                eff.stage("keepalive", responder::responder()).await,
                (state, stage),
            )
            .await;
        eff.contramap(&keepalive, "keepalive_handler", Inputs::<Void>::Network)
            .await
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
