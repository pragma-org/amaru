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
    blockfetch::{State, messages::Message},
    mux::MuxMessage,
    protocol::{
        Inputs, Miniprotocol, Outcome, PROTO_N2N_BLOCK_FETCH, ProtocolState, Responder, StageState,
        miniprotocol, outcome,
    },
};
use amaru_kernel::Point;
use pure_stage::{DeserializerGuards, Effects, StageRef, Void};

pub fn register_deserializers() -> DeserializerGuards {
    vec![pure_stage::register_data_deserializer::<BlockFetchResponder>().boxed()]
}

pub fn responder() -> Miniprotocol<State, BlockFetchResponder, Responder> {
    miniprotocol(PROTO_N2N_BLOCK_FETCH.responder())
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BlockFetchResponder {
    muxer: StageRef<MuxMessage>,
}

impl BlockFetchResponder {
    pub fn new(muxer: StageRef<MuxMessage>) -> (State, Self) {
        (State::Idle, Self { muxer })
    }
}

impl StageState<State, Responder> for BlockFetchResponder {
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
        match input {
            ResponderResult::RequestRange { .. } => Ok((Some(ResponderAction::NoBlocks), self)),
            ResponderResult::Done => Ok((None, self)),
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

    fn init(&self) -> anyhow::Result<(Outcome<Self::WireMsg, Self::Out>, Self)> {
        Ok((outcome(), *self))
    }

    fn network(
        &self,
        input: Self::WireMsg,
    ) -> anyhow::Result<(Outcome<Self::WireMsg, Self::Out>, Self)> {
        use Message::*;
        match (self, input) {
            (Self::Idle, RequestRange { from, through }) => Ok((
                outcome()
                    .want_next()
                    .result(ResponderResult::RequestRange { from, through }),
                Self::Busy,
            )),
            (Self::Idle, ClientDone) => Ok((
                outcome().want_next().result(ResponderResult::Done),
                Self::Done,
            )),
            (state, msg) => anyhow::bail!("unexpected message in state {:?}: {:?}", state, msg),
        }
    }

    fn local(&self, input: Self::Action) -> anyhow::Result<(Outcome<Self::WireMsg, Void>, Self)> {
        use ResponderAction::*;
        match (self, input) {
            (Self::Busy, StartBatch) => Ok((outcome().send(Message::StartBatch), Self::Streaming)),
            (Self::Busy, NoBlocks) => Ok((outcome().send(Message::NoBlocks), Self::Idle)),
            (Self::Streaming, Block(body)) => {
                Ok((outcome().send(Message::Block { body }), Self::Streaming))
            }
            (Self::Streaming, BatchDone) => Ok((outcome().send(Message::BatchDone), Self::Idle)),
            (state, action) => {
                anyhow::bail!("unexpected action in state {:?}: {:?}", state, action)
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq, Clone, serde::Serialize, serde::Deserialize)]
pub enum ResponderAction {
    StartBatch,
    NoBlocks,
    Block(Vec<u8>),
    BatchDone,
}

#[derive(Debug, PartialEq, Eq, Clone, serde::Serialize, serde::Deserialize)]
pub enum ResponderResult {
    RequestRange { from: Point, through: Point },
    Done,
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::protocol::Responder;

    #[test]
    #[expect(clippy::wildcard_enum_match_arm)]
    fn test_responder_protocol() {
        crate::blockfetch::spec::<Responder>().check(
            State::Idle,
            |msg| match msg {
                Message::NoBlocks => Some(ResponderAction::NoBlocks),
                Message::StartBatch => Some(ResponderAction::StartBatch),
                Message::Block { body } => Some(ResponderAction::Block(body.clone())),
                Message::BatchDone => Some(ResponderAction::BatchDone),
                _ => None,
            },
            |msg| msg.clone(),
        );
    }
}
