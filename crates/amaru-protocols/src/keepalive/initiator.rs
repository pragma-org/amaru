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
        Initiator, Inputs, Miniprotocol, Outcome, PROTO_N2N_KEEP_ALIVE, ProtocolState, StageState,
        miniprotocol, outcome,
    },
};
use pure_stage::{DeserializerGuards, Effects, StageRef, Void};

pub fn register_deserializers() -> DeserializerGuards {
    vec![
        pure_stage::register_data_deserializer::<InitiatorMessage>().boxed(),
        pure_stage::register_data_deserializer::<KeepAliveInitiator>().boxed(),
    ]
}

pub fn initiator() -> Miniprotocol<State, KeepAliveInitiator, Initiator> {
    miniprotocol(PROTO_N2N_KEEP_ALIVE)
}

/// Message sent to the handler to trigger periodic keep-alive sends
#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum InitiatorMessage {
    SendKeepAlive,
}

/// Message sent from the handler (for future use, e.g., RTT reporting)
#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct InitiatorResult {
    pub cookie: Cookie,
}

#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct KeepAliveInitiator {
    cookie: Cookie,
    muxer: StageRef<MuxMessage>,
}

impl KeepAliveInitiator {
    pub fn new(muxer: StageRef<MuxMessage>) -> (State, Self) {
        (
            State::Idle,
            Self {
                cookie: Cookie::new(),
                muxer,
            },
        )
    }
}

impl AsRef<StageRef<MuxMessage>> for KeepAliveInitiator {
    fn as_ref(&self) -> &StageRef<MuxMessage> {
        &self.muxer
    }
}

impl StageState<State, Initiator> for KeepAliveInitiator {
    type LocalIn = InitiatorMessage;

    async fn local(
        self,
        proto: &State,
        input: Self::LocalIn,
        _eff: &Effects<Inputs<Self::LocalIn>>,
    ) -> anyhow::Result<(Option<InitiatorAction>, Self)> {
        use State::*;

        match (proto, input) {
            (Idle, InitiatorMessage::SendKeepAlive) => {
                Ok((Some(InitiatorAction::SendKeepAlive(self.cookie)), self))
            }
            (this, input) => anyhow::bail!("invalid state: {:?} <- {:?}", this, input),
        }
    }

    async fn network(
        mut self,
        _proto: &State,
        input: InitiatorResult,
        eff: &Effects<Inputs<Self::LocalIn>>,
    ) -> anyhow::Result<(Option<InitiatorAction>, Self)> {
        // After receiving a response, increment cookie and schedule next send
        self.cookie = input.cookie.next();

        // Schedule next keep-alive after 1 second by sending a message to ourselves
        eff.wait(std::time::Duration::from_secs(1)).await;
        eff.send(eff.me_ref(), Inputs::Local(InitiatorMessage::SendKeepAlive))
            .await;

        Ok((None, self))
    }
}

impl ProtocolState<Initiator> for State {
    type WireMsg = Message;
    type Action = InitiatorAction;
    type Out = InitiatorResult;

    fn init(&self) -> anyhow::Result<(Outcome<Self::WireMsg, Self::Out>, Self)> {
        // On init, trigger the first KeepAlive send
        Ok((
            outcome().result(InitiatorResult {
                cookie: Cookie::new(),
            }),
            *self,
        ))
    }

    fn network(
        &self,
        input: Self::WireMsg,
    ) -> anyhow::Result<(Outcome<Self::WireMsg, Self::Out>, Self)> {
        use State::*;

        Ok(match (self, input) {
            (Waiting, Message::ResponseKeepAlive(cookie)) => (
                outcome().want_next().result(InitiatorResult { cookie }),
                Idle,
            ),
            (this, input) => anyhow::bail!("invalid state: {:?} <- {:?}", this, input),
        })
    }

    fn local(&self, input: Self::Action) -> anyhow::Result<(Outcome<Self::WireMsg, Void>, Self)> {
        use State::*;

        Ok(match (self, input) {
            (Idle, InitiatorAction::SendKeepAlive(cookie)) => {
                (outcome().send(Message::KeepAlive(cookie)), Waiting)
            }
            (this, input) => anyhow::bail!("invalid state: {:?} <- {:?}", this, input),
        })
    }
}

#[derive(Debug)]
pub enum InitiatorAction {
    SendKeepAlive(Cookie),
}

#[cfg(test)]
#[expect(clippy::wildcard_enum_match_arm)]
pub mod tests {
    use crate::keepalive::State;
    use crate::keepalive::initiator::InitiatorAction;
    use crate::keepalive::messages::Message;
    use crate::protocol::{Initiator, Role};

    #[test]
    fn test_initiator_protocol() {
        crate::keepalive::spec::<Initiator>().check(
            State::Idle,
            Role::Initiator,
            |msg| match msg {
                Message::KeepAlive(cookie) => Some(InitiatorAction::SendKeepAlive(*cookie)),
                _ => None,
            },
            |msg| *msg,
        );
    }
}
