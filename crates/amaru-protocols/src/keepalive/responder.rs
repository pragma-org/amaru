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

use crate::{
    keepalive::{
        State,
        messages::{Cookie, Message},
    },
    mux::MuxMessage,
    protocol::{
        Inputs, Miniprotocol, Outcome, PROTO_N2N_KEEP_ALIVE, ProtocolState, Responder, StageState,
        miniprotocol, outcome,
    },
};
use pure_stage::{DeserializerGuards, Effects, StageRef, Void};

pub fn register_deserializers() -> DeserializerGuards {
    vec![pure_stage::register_data_deserializer::<KeepAliveResponder>().boxed()]
}

pub fn responder() -> Miniprotocol<State, KeepAliveResponder, Responder> {
    miniprotocol(PROTO_N2N_KEEP_ALIVE.responder())
}

#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct KeepAliveResponder {
    muxer: StageRef<MuxMessage>,
}

impl KeepAliveResponder {
    pub fn new(muxer: StageRef<MuxMessage>) -> (State, Self) {
        (State::Idle, Self { muxer })
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
        _eff: &Effects<Inputs<Self::LocalIn>>,
    ) -> anyhow::Result<(Option<ResponderAction>, Self)> {
        Ok((Some(ResponderAction::SendResponse(input.cookie)), self))
    }

    fn muxer(&self) -> &StageRef<MuxMessage> {
        &self.muxer
    }
}

impl ProtocolState<Responder> for State {
    type WireMsg = Message;
    type Action = ResponderAction;
    type Out = ResponderResult;

    fn init(&self) -> anyhow::Result<(Outcome<Self::WireMsg, Self::Out>, Self)> {
        Ok((outcome(), *self))
    }

    fn network(
        &self,
        input: Self::WireMsg,
    ) -> anyhow::Result<(Outcome<Self::WireMsg, Self::Out>, Self)> {
        use State::*;

        Ok(match (self, input) {
            (Idle, Message::KeepAlive(cookie)) => (
                outcome().want_next().result(ResponderResult { cookie }),
                Waiting,
            ),
            (this, input) => anyhow::bail!("invalid state: {:?} <- {:?}", this, input),
        })
    }

    fn local(&self, input: Self::Action) -> anyhow::Result<(Outcome<Self::WireMsg, Void>, Self)> {
        use State::*;

        Ok(match (self, input) {
            (Waiting, ResponderAction::SendResponse(cookie)) => {
                (outcome().send(Message::ResponseKeepAlive(cookie)), Idle)
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
pub struct ResponderResult {
    pub cookie: Cookie,
}

#[cfg(test)]
pub mod tests {
    use crate::keepalive::State;
    use crate::keepalive::messages::Message;
    use crate::keepalive::responder::ResponderAction;
    use crate::protocol::Responder;

    #[test]
    fn test_responder_protocol() {
        crate::keepalive::spec::<Responder>().check(
            State::Idle,
            |msg| match msg {
                // ResponseKeepAlive is sent by responder (local action)
                Message::ResponseKeepAlive(cookie) => Some(ResponderAction::SendResponse(*cookie)),
                // KeepAlive is received from initiator (network message)
                Message::KeepAlive(_) => None,
                Message::Done => None,
            },
            |msg| *msg,
        );
    }
}
