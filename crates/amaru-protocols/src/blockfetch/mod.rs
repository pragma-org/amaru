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
    blockfetch::messages::Message,
    mux::MuxMessage,
    protocol::{
        Initiator, Inputs, Miniprotocol, Outcome, PROTO_N2N_BLOCK_FETCH, ProtocolState, Responder,
        StageState, miniprotocol, outcome,
    },
};
use amaru_kernel::Point;
use pure_stage::{CallRef, DeserializerGuards, Effects, StageRef, Void};
use std::{collections::VecDeque, mem};

mod messages;

pub fn register_deserializers() -> DeserializerGuards {
    vec![
        pure_stage::register_data_deserializer::<BlockFetchMessage>().boxed(),
        pure_stage::register_data_deserializer::<BlockFetch>().boxed(),
        pure_stage::register_data_deserializer::<Blocks>().boxed(),
    ]
}

pub fn initiator() -> Miniprotocol<State, BlockFetch, Initiator> {
    miniprotocol(PROTO_N2N_BLOCK_FETCH)
}

#[derive(Debug, PartialEq, Eq, Clone, serde::Serialize, serde::Deserialize)]
pub struct Blocks {
    blocks: Vec<Vec<u8>>,
}

#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum BlockFetchMessage {
    RequestRange {
        from: Point,
        through: Point,
        cr: CallRef<Blocks>,
    },
}

#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BlockFetch {
    muxer: StageRef<MuxMessage>,
    queue: VecDeque<(Point, Point, CallRef<Blocks>)>,
    blocks: Vec<Vec<u8>>,
}

impl BlockFetch {
    pub fn new(muxer: StageRef<MuxMessage>) -> Self {
        Self {
            muxer,
            queue: VecDeque::new(),
            blocks: Vec::new(),
        }
    }
}

impl AsRef<StageRef<MuxMessage>> for BlockFetch {
    fn as_ref(&self) -> &StageRef<MuxMessage> {
        &self.muxer
    }
}

impl StageState<State, Initiator> for BlockFetch {
    type LocalIn = BlockFetchMessage;

    async fn local(
        mut self,
        _proto: &State,
        input: Self::LocalIn,
        _eff: &Effects<Inputs<Self::LocalIn>>,
    ) -> anyhow::Result<(Option<InitiatorAction>, Self)> {
        match input {
            BlockFetchMessage::RequestRange { from, through, cr } => {
                let action = self
                    .queue
                    .is_empty()
                    .then_some(InitiatorAction::RequestRange { from, through });
                self.queue.push_back((from, through, cr));
                Ok((action, self))
            }
        }
    }

    async fn network(
        mut self,
        _proto: &State,
        input: InitiatorResult,
        eff: &Effects<Inputs<Self::LocalIn>>,
    ) -> anyhow::Result<(Option<InitiatorAction>, Self)> {
        let queued = match input {
            InitiatorResult::NoBlocks => {
                let (_, _, cr) = self.queue.pop_front().expect("queue is empty");
                eff.respond(cr, Blocks { blocks: Vec::new() }).await;
                self.queue.front()
            }
            InitiatorResult::Block(body) => {
                self.blocks.push(body);
                None
            }
            InitiatorResult::Done => {
                let (_, _, cr) = self.queue.pop_front().expect("queue is empty");
                let blocks = mem::take(&mut self.blocks);
                eff.respond(cr, Blocks { blocks }).await;
                self.queue.front()
            }
        };
        let action = queued.map(|(from, through, _)| InitiatorAction::RequestRange {
            from: *from,
            through: *through,
        });
        Ok((action, self))
    }
}

impl StageState<State, Responder> for BlockFetch {
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
            ResponderResult::RequestRange { .. } => todo!(),
            ResponderResult::Done => todo!(),
        }
    }
}

#[derive(Debug, PartialEq, Eq, Clone, serde::Serialize, serde::Deserialize)]
pub enum InitiatorResult {
    NoBlocks,
    Block(Vec<u8>),
    Done,
}

#[derive(Debug, PartialEq, Eq, Clone, serde::Serialize, serde::Deserialize)]
pub enum ResponderResult {
    RequestRange { from: Point, through: Point },
    Done,
}

#[derive(Debug, PartialEq, Eq, Clone, serde::Serialize, serde::Deserialize)]
pub enum InitiatorAction {
    RequestRange { from: Point, through: Point },
    ClientDone,
}

#[derive(Debug, PartialEq, Eq, Clone, serde::Serialize, serde::Deserialize)]
pub enum ResponderAction {
    StartBatch,
    NoBlocks,
    Block(Vec<u8>),
    BatchDone,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub enum State {
    Idle,
    Busy,
    Streaming,
    Done,
}

impl ProtocolState<Initiator> for State {
    type WireMsg = messages::Message;
    type Action = InitiatorAction;
    type Out = InitiatorResult;

    fn init(&self) -> anyhow::Result<(Outcome<Self::WireMsg, Self::Out>, Self)> {
        Ok((outcome(), *self))
    }

    fn network(
        &self,
        input: Self::WireMsg,
    ) -> anyhow::Result<(Outcome<Self::WireMsg, Self::Out>, Self)> {
        use messages::Message::*;
        match (self, input) {
            (Self::Busy, StartBatch) => Ok((outcome().want_next(), Self::Streaming)),
            (Self::Busy, NoBlocks) => Ok((outcome().want_next(), Self::Idle)),
            (Self::Streaming, Block { body }) => Ok((
                outcome().want_next().result(InitiatorResult::Block(body)),
                Self::Streaming,
            )),
            (Self::Streaming, BatchDone) => Ok((
                outcome().want_next().result(InitiatorResult::Done),
                Self::Idle,
            )),
            (state, msg) => anyhow::bail!("unexpected message in state {:?}: {:?}", state, msg),
        }
    }

    fn local(&self, input: Self::Action) -> anyhow::Result<(Outcome<Self::WireMsg, Void>, Self)> {
        use InitiatorAction::*;
        match (self, input) {
            (Self::Idle, RequestRange { from, through }) => Ok((
                outcome().send(Message::RequestRange { from, through }),
                Self::Busy,
            )),
            (Self::Idle, ClientDone) => Ok((outcome().send(Message::ClientDone), Self::Done)),
            (state, action) => {
                anyhow::bail!("unexpected action in state {:?}: {:?}", state, action)
            }
        }
    }
}

impl ProtocolState<Responder> for State {
    type WireMsg = messages::Message;
    type Action = ResponderAction;
    type Out = ResponderResult;

    fn init(&self) -> anyhow::Result<(Outcome<Self::WireMsg, Self::Out>, Self)> {
        Ok((outcome(), *self))
    }

    fn network(
        &self,
        input: Self::WireMsg,
    ) -> anyhow::Result<(Outcome<Self::WireMsg, Self::Out>, Self)> {
        use messages::Message::*;
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
