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

use std::collections::VecDeque;

use amaru_kernel::{NetworkPoint, Peer, Point, RawBlock, cardano::network_block::NetworkBlock, utils::debug_bytes};
use amaru_observability::{Instrument, debug, debug_span, warn};
use amaru_ouroboros::ConnectionId;
use amaru_pure_stage::{DeserializerGuards, Effects, StageRef, Void};

use crate::{
    blockfetch::{State, messages::Message, responder::MAX_FETCHED_BLOCKS},
    mux::MuxMessage,
    protocol::{
        Initiator, Inputs, Miniprotocol, Outcome, PROTO_N2N_BLOCK_FETCH, ProtocolState, StageState, miniprotocol,
        outcome,
    },
};

pub fn register_deserializers() -> DeserializerGuards {
    vec![
        amaru_pure_stage::register_data_deserializer::<BlockFetchInitiator>().boxed(),
        amaru_pure_stage::register_data_deserializer::<(State, BlockFetchInitiator)>().boxed(),
        amaru_pure_stage::register_data_deserializer::<BlockFetchMessage>().boxed(),
        amaru_pure_stage::register_data_deserializer::<Blocks>().boxed(),
    ]
}

pub fn initiator() -> Miniprotocol<State, BlockFetchInitiator, Initiator> {
    miniprotocol(PROTO_N2N_BLOCK_FETCH)
}

#[derive(PartialEq, Clone, serde::Serialize, serde::Deserialize)]
pub enum Blocks {
    /// Peer responded that it has no blocks in the requested range.
    NoBlocks(u64, Peer),
    /// Manager found no initiating connections for this request id.
    NoPeersAvailable(u64),
    /// Peers to which the manager dispatched this fetch request (for performance scoring).
    PeersAsked(u64, Vec<Peer>),
    Block(u64, Peer, NetworkBlock),
    Done(u64),
}

impl std::fmt::Debug for Blocks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoBlocks(id, peer) => f.debug_tuple("NoBlocks").field(id).field(peer).finish(),
            Self::NoPeersAvailable(id) => f.debug_tuple("NoPeersAvailable").field(id).finish(),
            Self::PeersAsked(id, peers) => f.debug_tuple("PeersAsked").field(id).field(peers).finish(),
            Self::Block(id, peer, block) => {
                f.debug_tuple("Block").field(id).field(peer).field(&debug_bytes(block.as_slice(), 80)).finish()
            }
            Self::Done(id) => f.debug_tuple("Done").field(id).finish(),
        }
    }
}

/// Message that can be sent by an internal stage to request blocks for range of points.
#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum BlockFetchMessage {
    RequestRange { from: Point, through: Point, id: u64, cr: StageRef<Blocks> },
}

#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BlockFetchInitiator {
    muxer: StageRef<MuxMessage>,
    peer: Peer,
    conn_id: ConnectionId,
    /// Queue of requests that have been received but not yet answered.
    ///
    /// Note that the first two elements of the queue have already been sent
    /// to the network (pipelining).
    queue: VecDeque<(Point, Point, u64, StageRef<Blocks>, usize)>,
}

impl BlockFetchInitiator {
    /// Create a new BlockFetchInitiator instance for a given peer, using a given connection.
    /// Returns the initial state and the initiator instance.
    pub fn new(muxer: StageRef<MuxMessage>, peer: Peer, conn_id: ConnectionId) -> (State, Self) {
        (State::Idle, Self { muxer, peer, conn_id, queue: VecDeque::new() })
    }
}

impl StageState<State, Initiator> for BlockFetchInitiator {
    type LocalIn = BlockFetchMessage;

    async fn local(
        mut self,
        proto: &State,
        input: Self::LocalIn,
        _eff: &Effects<Inputs<Self::LocalIn>>,
    ) -> anyhow::Result<(Option<InitiatorAction>, Self)> {
        match input {
            BlockFetchMessage::RequestRange { from, through, id, cr } => {
                let action = (*proto == State::Idle).then_some(InitiatorAction::RequestRange {
                    from: from.to_network_point(),
                    through: through.to_network_point(),
                });
                if self.queue.len() > 1 {
                    debug!(protocols::blockfetch::initiator::DROPPED_SLOW_PEER, peer = &self.peer);
                }
                self.queue.truncate(1);
                self.queue.push_back((from, through, id, cr, MAX_FETCHED_BLOCKS));
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
        let span = debug_span!(
            protocols::blockfetch::initiator::BLOCKFETCH_INITIATOR_PROTOCOL,
            message_type = input.message_type()
        );
        async move {
            let queued = match input {
                InitiatorResult::Initialize => None,
                InitiatorResult::NoBlocks => {
                    let (_, _, id, cr, _) = self.queue.pop_front().expect("queue must not be empty");
                    eff.send(&cr, Blocks::NoBlocks(id, self.peer.clone())).await;
                    self.queue.front()
                }
                InitiatorResult::Block(body) => {
                    if let Ok(network_block) = NetworkBlock::try_from(RawBlock::from(body.as_slice())) {
                        if let Some((_, _, id, cr, remaining_blocks)) = self.queue.front_mut() {
                            if *remaining_blocks == 0 {
                                warn!(
                                    protocols::blockfetch::initiator::PROTOCOL_VIOLATION,
                                    reason = "too_many_blocks",
                                    max_blocks = MAX_FETCHED_BLOCKS
                                );
                                return eff.terminate().await;
                            }
                            *remaining_blocks -= 1;
                            let id = *id;
                            eff.send(cr, Blocks::Block(id, self.peer.clone(), network_block)).await;
                        } else {
                            warn!(protocols::blockfetch::initiator::PROTOCOL_VIOLATION, reason = "no_pending_request");
                            return eff.terminate().await;
                        }
                    } else {
                        warn!(
                            protocols::blockfetch::initiator::PROTOCOL_VIOLATION,
                            reason = "invalid_cbor",
                            bytes = body.len()
                        );
                        return eff.terminate().await;
                    }
                    None
                }
                InitiatorResult::Done => {
                    let (_, _, id, cr, _) = self.queue.pop_front().expect("queue must not be empty");
                    eff.send(&cr, Blocks::Done(id)).await;
                    self.queue.front()
                }
            };
            let action = queued.map(|(from, through, _, _, _)| InitiatorAction::RequestRange {
                from: from.to_network_point(),
                through: through.to_network_point(),
            });
            Ok((action, self))
        }
        .instrument(span)
        .await
    }

    fn muxer(&self) -> &StageRef<MuxMessage> {
        &self.muxer
    }
}

impl ProtocolState<Initiator> for State {
    type WireMsg = Message;
    type Action = InitiatorAction;
    type Out = InitiatorResult;
    type Error = Void;

    fn init(&self) -> anyhow::Result<(Outcome<Self::WireMsg, Self::Out, Self::Error>, Self)> {
        Ok((outcome().result(InitiatorResult::Initialize), *self))
    }

    fn network(&self, input: Self::WireMsg) -> anyhow::Result<(Outcome<Self::WireMsg, Self::Out, Self::Error>, Self)> {
        let _span = debug_span!(
            protocols::blockfetch::initiator::BLOCKFETCH_INITIATOR_STAGE,
            message_type = input.message_type()
        );
        let _guard = _span.enter();
        use Message::*;
        match (self, input) {
            (Self::Busy, StartBatch) => Ok((outcome().want_next(), Self::Streaming)),
            (Self::Busy, NoBlocks) => Ok((outcome().result(InitiatorResult::NoBlocks), Self::Idle)),
            (Self::Streaming, Block { body }) => {
                Ok((outcome().want_next().result(InitiatorResult::Block(body)), Self::Streaming))
            }
            (Self::Streaming, BatchDone) => Ok((outcome().result(InitiatorResult::Done), Self::Idle)),
            (state, msg) => anyhow::bail!("unexpected message in state {:?}: {:?}", state, msg),
        }
    }

    fn local(&self, input: Self::Action) -> anyhow::Result<(Outcome<Self::WireMsg, Void, Self::Error>, Self)> {
        use InitiatorAction::*;
        match (self, input) {
            (Self::Idle, RequestRange { from, through }) => {
                Ok((outcome().send(Message::RequestRange { from, through }).want_next(), Self::Busy))
            }
            (Self::Idle, ClientDone) => Ok((outcome().send(Message::ClientDone), Self::Done)),
            (state, action) => {
                anyhow::bail!("unexpected action in state {:?}: {:?}", state, action)
            }
        }
    }
}

/// Result of the initiator protocol step, to be used by the local stage.
#[derive(Debug, PartialEq, Eq, Clone, serde::Serialize, serde::Deserialize)]
pub enum InitiatorResult {
    Initialize,
    NoBlocks,
    Block(Vec<u8>),
    Done,
}

impl InitiatorResult {
    fn message_type(&self) -> &'static str {
        match self {
            Self::Initialize => "Initialize",
            Self::NoBlocks => "NoBlocks",
            Self::Block(_) => "Block",
            Self::Done => "Done",
        }
    }
}

/// Outcome action of the local stage, to be used by the initiator protocol stage.
#[derive(Debug, PartialEq, Eq, Clone, serde::Serialize, serde::Deserialize)]
pub enum InitiatorAction {
    RequestRange { from: NetworkPoint, through: NetworkPoint },
    ClientDone,
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::protocol::Initiator;

    #[test]
    #[expect(clippy::wildcard_enum_match_arm)]
    fn test_initiator_protocol() {
        crate::blockfetch::spec::<Initiator>().check(State::Idle, |msg| match msg {
            Message::RequestRange { from, through } => {
                Some(InitiatorAction::RequestRange { from: *from, through: *through })
            }
            Message::ClientDone => Some(InitiatorAction::ClientDone),
            _ => None,
        });
    }
}
