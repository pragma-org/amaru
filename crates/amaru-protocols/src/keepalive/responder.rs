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

use amaru_kernel::Peer;
use amaru_observability::{Instrument, debug_span};
use amaru_ouroboros::{ConnectionId, RemoteProtocol};
use amaru_pure_stage::{DeserializerGuards, Effects, StageRef, Void};

use crate::{
    keepalive::{
        State,
        messages::{Cookie, Message},
    },
    mux::MuxMessage,
    peer_tracking_effects::PeerTrack,
    protocol::{
        Inputs, Miniprotocol, Outcome, PROTO_N2N_KEEP_ALIVE, ProtocolState, Responder, StageState, miniprotocol,
        outcome,
    },
};

pub fn register_deserializers() -> DeserializerGuards {
    vec![
        amaru_pure_stage::register_data_deserializer::<KeepAliveResponder>().boxed(),
        amaru_pure_stage::register_data_deserializer::<(State, KeepAliveResponder)>().boxed(),
    ]
}

pub fn responder() -> Miniprotocol<State, KeepAliveResponder, Responder> {
    miniprotocol(PROTO_N2N_KEEP_ALIVE.responder())
}

#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct KeepAliveResponder {
    muxer: StageRef<MuxMessage>,
    peer: Peer,
    conn_id: ConnectionId,
}

impl KeepAliveResponder {
    pub fn new(muxer: StageRef<MuxMessage>, peer: Peer, conn_id: ConnectionId) -> (State, Self) {
        (State::Idle, Self { muxer, peer, conn_id })
    }
}

impl StageState<State, Responder> for KeepAliveResponder {
    type LocalIn = Void;

    async fn local(
        self,
        _proto: &State,
        input: Self::LocalIn,
        _eff: &Effects<Inputs<Self::LocalIn>>,
    ) -> anyhow::Result<(Option<ResponderAction>, Self)> {
        match input {}
    }

    async fn network(
        self,
        _proto: &State,
        input: ResponderResult,
        eff: &Effects<Inputs<Self::LocalIn>>,
    ) -> anyhow::Result<(Option<ResponderAction>, Self)> {
        match input {
            ResponderResult::KeepAlive { cookie } => {
                let n = cookie.as_u16();
                let peer = self.peer;
                let conn_id = self.conn_id;
                async move {
                    PeerTrack::new(eff).note_remote_protocol(peer, conn_id, RemoteProtocol::KeepAlive, true).await;
                    Ok((Some(ResponderAction::SendResponse(cookie)), self))
                }
                .instrument(debug_span!(protocols::keepalive::responder::KEEPALIVE_RESPONDER_STAGE, cookie = n).into())
                .await
            }
            ResponderResult::Done => {
                PeerTrack::new(eff)
                    .note_remote_protocol(self.peer, self.conn_id, RemoteProtocol::KeepAlive, false)
                    .await;
                Ok((None, self))
            }
        }
    }

    fn muxer(&self) -> &StageRef<MuxMessage> {
        &self.muxer
    }
}

impl ProtocolState<Responder> for State {
    type WireMsg = Message;
    type Action = ResponderAction;
    type Out = ResponderResult;
    type Error = Void;

    fn init(&self) -> anyhow::Result<(Outcome<Self::WireMsg, Self::Out, Self::Error>, Self)> {
        Ok((outcome().want_next(), *self))
    }

    fn network(&self, input: Self::WireMsg) -> anyhow::Result<(Outcome<Self::WireMsg, Self::Out, Self::Error>, Self)> {
        let _span = debug_span!(
            protocols::keepalive::responder::KEEPALIVE_RESPONDER_PROTOCOL,
            message_type = input.message_type().to_string()
        );
        let _guard = _span.enter();
        use State::*;

        Ok(match (self, input) {
            (Idle, Message::KeepAlive(cookie)) => (outcome().result(ResponderResult::KeepAlive { cookie }), Waiting),
            (Idle, Message::Done) => (outcome().result(ResponderResult::Done).want_next(), Idle),
            (this, input) => anyhow::bail!("invalid state: {:?} <- {:?}", this, input),
        })
    }

    fn local(&self, input: Self::Action) -> anyhow::Result<(Outcome<Self::WireMsg, Void, Self::Error>, Self)> {
        use State::*;

        Ok(match (self, input) {
            (Waiting, ResponderAction::SendResponse(cookie)) => {
                (outcome().send(Message::ResponseKeepAlive(cookie)).want_next(), Idle)
            }
            (this, input) => anyhow::bail!("invalid state: {:?} <- {:?}", this, input),
        })
    }
}

#[derive(Debug)]
pub enum ResponderAction {
    SendResponse(Cookie),
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ResponderResult {
    KeepAlive { cookie: Cookie },
    Done,
}

#[cfg(test)]
pub mod tests {
    use crate::{
        keepalive::{State, messages::Message, responder::ResponderAction},
        protocol::Responder,
    };

    #[test]
    fn test_responder_protocol() {
        crate::keepalive::spec::<Responder>().check(State::Idle, |msg| match msg {
            // ResponseKeepAlive is sent by responder (local action)
            Message::ResponseKeepAlive(cookie) => Some(ResponderAction::SendResponse(*cookie)),
            // KeepAlive is received from initiator (network message)
            Message::KeepAlive(_) => None,
            Message::Done => None,
        });
    }
}
