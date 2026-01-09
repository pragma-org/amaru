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
        Initiator, Inputs, Miniprotocol, Outcome, PROTO_N2N_BLOCK_FETCH, ProtocolState, StageState,
        miniprotocol, outcome,
    },
};
use amaru_kernel::{Point, peer::Peer};
use amaru_ouroboros::ConnectionId;
use pure_stage::{DeserializerGuards, Effects, StageRef, Void};
use std::{collections::VecDeque, mem};

pub fn register_deserializers() -> DeserializerGuards {
    vec![
        pure_stage::register_data_deserializer::<BlockFetchInitiator>().boxed(),
        pure_stage::register_data_deserializer::<BlockFetchMessage>().boxed(),
        pure_stage::register_data_deserializer::<Blocks>().boxed(),
    ]
}

pub fn initiator() -> Miniprotocol<State, BlockFetchInitiator, Initiator> {
    miniprotocol(PROTO_N2N_BLOCK_FETCH)
}

#[derive(Default, PartialEq, Eq, Clone, serde::Serialize, serde::Deserialize)]
pub struct Blocks {
    pub blocks: Vec<Vec<u8>>,
}

impl std::fmt::Debug for Blocks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Blocks")
            .field("blocks", &self.blocks.len())
            .field("first_block", &self.blocks.first().map(|b| b.len()))
            .finish()
    }
}

#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum BlockFetchMessage {
    RequestRange {
        from: Point,
        through: Point,
        cr: StageRef<Blocks>,
    },
}

#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BlockFetchInitiator {
    muxer: StageRef<MuxMessage>,
    peer: Peer,
    conn_id: ConnectionId,
    queue: VecDeque<(Point, Point, StageRef<Blocks>)>,
    blocks: Vec<Vec<u8>>,
}

impl BlockFetchInitiator {
    pub fn new(muxer: StageRef<MuxMessage>, peer: Peer, conn_id: ConnectionId) -> (State, Self) {
        (
            State::Idle,
            Self {
                muxer,
                peer,
                conn_id,
                queue: VecDeque::new(),
                blocks: Vec::new(),
            },
        )
    }
}

impl StageState<State, Initiator> for BlockFetchInitiator {
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

    #[expect(clippy::expect_used)]
    async fn network(
        mut self,
        _proto: &State,
        input: InitiatorResult,
        eff: &Effects<Inputs<Self::LocalIn>>,
    ) -> anyhow::Result<(Option<InitiatorAction>, Self)> {
        let queued = match input {
            InitiatorResult::NoBlocks => {
                let (_, _, cr) = self.queue.pop_front().expect("queue is empty");
                eff.send(&cr, Blocks { blocks: Vec::new() }).await;
                self.queue.front()
            }
            InitiatorResult::Block(body) => {
                self.blocks.push(body);
                None
            }
            InitiatorResult::Done => {
                let (_, _, cr) = self.queue.pop_front().expect("queue is empty");
                let blocks = mem::take(&mut self.blocks);
                eff.send(&cr, Blocks { blocks }).await;
                self.queue.front()
            }
        };
        let action = queued.map(|(from, through, _)| InitiatorAction::RequestRange {
            from: *from,
            through: *through,
        });
        Ok((action, self))
    }

    fn muxer(&self) -> &StageRef<MuxMessage> {
        &self.muxer
    }
}

impl ProtocolState<Initiator> for State {
    type WireMsg = Message;
    type Action = InitiatorAction;
    type Out = InitiatorResult;

    fn init(&self) -> anyhow::Result<(Outcome<Self::WireMsg, Self::Out>, Self)> {
        Ok((outcome(), *self))
    }

    fn network(
        &self,
        input: Self::WireMsg,
    ) -> anyhow::Result<(Outcome<Self::WireMsg, Self::Out>, Self)> {
        use Message::*;
        match (self, input) {
            (Self::Busy, StartBatch) => Ok((outcome().want_next(), Self::Streaming)),
            (Self::Busy, NoBlocks) => Ok((
                outcome().want_next().result(InitiatorResult::NoBlocks),
                Self::Idle,
            )),
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

#[derive(Debug, PartialEq, Eq, Clone, serde::Serialize, serde::Deserialize)]
pub enum InitiatorResult {
    NoBlocks,
    Block(Vec<u8>),
    Done,
}

#[derive(Debug, PartialEq, Eq, Clone, serde::Serialize, serde::Deserialize)]
pub enum InitiatorAction {
    RequestRange { from: Point, through: Point },
    ClientDone,
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::protocol::Initiator;

    #[test]
    fn test_initiator_protocol() {
        crate::blockfetch::spec::<Initiator>().check(
            State::Idle,
            |msg| match msg {
                Message::RequestRange { from, through } => Some(InitiatorAction::RequestRange {
                    from: *from,
                    through: *through,
                }),
                _ => None,
            },
            |msg| msg.clone(),
        );
    }
}
