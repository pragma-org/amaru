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
    chainsync::messages::{HeaderContent, Message},
    mux::MuxMessage,
    protocol::{
        Inputs, Miniprotocol, Outcome, PROTO_N2N_CHAIN_SYNC, ProtocolState, Responder, StageState,
        miniprotocol, outcome,
    },
    store_effects::Store,
};
use amaru_kernel::{
    BlockHeader, IsHeader, Point, peer::Peer, protocol_messages::tip::Tip, to_cbor,
};
use amaru_ouroboros::{ConnectionId, ReadOnlyChainStore};
use anyhow::{Context, ensure};
use pure_stage::{DeserializerGuards, Effects, StageRef, Void};
use std::cmp::Reverse;

pub fn register_deserializers() -> DeserializerGuards {
    vec![
        pure_stage::register_data_deserializer::<ResponderMessage>().boxed(),
        pure_stage::register_data_deserializer::<ChainSyncResponder>().boxed(),
    ]
}

pub fn responder() -> Miniprotocol<ResponderState, ChainSyncResponder, Responder> {
    miniprotocol(PROTO_N2N_CHAIN_SYNC.responder())
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ResponderMessage {
    NewTip(Tip),
}

#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ChainSyncResponder {
    upstream: Tip,
    peer: Peer,
    pointer: Point,
    conn_id: ConnectionId,
    muxer: StageRef<MuxMessage>,
}

impl ChainSyncResponder {
    pub fn new(
        upstream: Tip,
        peer: Peer,
        conn_id: ConnectionId,
        muxer: StageRef<MuxMessage>,
    ) -> (ResponderState, Self) {
        (
            ResponderState::Idle {
                send_rollback: false,
            },
            Self {
                upstream,
                peer,
                pointer: Point::Origin,
                conn_id,
                muxer,
            },
        )
    }
}

impl StageState<ResponderState, Responder> for ChainSyncResponder {
    type LocalIn = ResponderMessage;

    async fn local(
        mut self,
        proto: &ResponderState,
        input: Self::LocalIn,
        eff: &Effects<Inputs<Self::LocalIn>>,
    ) -> anyhow::Result<(Option<ResponderAction>, Self)> {
        match input {
            ResponderMessage::NewTip(tip) => {
                self.upstream = tip;
                let action = next_header(
                    *proto,
                    self.pointer,
                    &Store::new(eff.clone()),
                    self.upstream,
                )
                .context("failed to get next header")?;
                Ok((action, self))
            }
        }
    }

    async fn network(
        self,
        proto: &ResponderState,
        input: ResponderResult,
        eff: &Effects<Inputs<Self::LocalIn>>,
    ) -> anyhow::Result<(Option<ResponderAction>, Self)> {
        match input {
            ResponderResult::FindIntersect(points) => {
                let action = intersect(points, &Store::new(eff.clone()), self.upstream)
                    .context("failed to find intersection")?;
                Ok((Some(action), self))
            }
            ResponderResult::RequestNext => {
                let action = next_header(
                    *proto,
                    self.pointer,
                    &Store::new(eff.clone()),
                    self.upstream,
                )
                .context("failed to get next header")?;
                Ok((action, self))
            }
            ResponderResult::Done => {
                tracing::info!("peer stopped chainsync");
                Ok((None, self))
            }
        }
    }

    fn muxer(&self) -> &StageRef<MuxMessage> {
        &self.muxer
    }
}

fn next_header(
    state: ResponderState,
    pointer: Point,
    store: &dyn ReadOnlyChainStore<BlockHeader>,
    tip: Tip,
) -> anyhow::Result<Option<ResponderAction>> {
    match state {
        ResponderState::CanAwait {
            send_rollback: true,
        } => return Ok(Some(ResponderAction::RollBackward(pointer, tip))),
        ResponderState::MustReply | ResponderState::CanAwait { .. } => {}
        ResponderState::Idle { .. } | ResponderState::Intersect | ResponderState::Done => {
            return Ok(None);
        }
    };
    if pointer == tip.point() {
        return Ok((matches!(state, ResponderState::CanAwait { .. }))
            .then_some(ResponderAction::AwaitReply));
    }
    if store.load_from_best_chain(&pointer).is_none() {
        // client is on a different fork, we need to roll backward
        let header = store
            .load_header(&pointer.hash())
            .ok_or_else(|| anyhow::anyhow!("remote pointer not found"))?;
        for header in store.ancestors(header) {
            if store.load_from_best_chain(&header.point()).is_some() {
                return Ok(Some(ResponderAction::RollBackward(header.point(), tip)));
            }
        }
        anyhow::bail!("no overlap found between client pointer chain and stored best chain");
    }
    // pointer is on the best chain, we need to roll forward
    Ok(store
        .next_best_chain(&pointer)
        .and_then(|point| store.load_header(&point.hash()))
        .map(|header| {
            ResponderAction::RollForward(
                HeaderContent {
                    variant: 6, // FIXME
                    byron_prefix: None,
                    cbor: to_cbor(&header),
                },
                tip,
            )
        }))
}

fn intersect(
    mut points: Vec<Point>,
    store: &dyn ReadOnlyChainStore<BlockHeader>,
    tip: Tip,
) -> anyhow::Result<ResponderAction> {
    if points.is_empty() {
        return Ok(ResponderAction::IntersectNotFound(tip));
    }
    points.sort_by_key(|p| Reverse(*p));
    let header = store
        .load_header(&tip.hash())
        .ok_or_else(|| anyhow::anyhow!("tip not found"))?;
    for header in store.ancestors(header) {
        let point = header.point();
        if points.contains(&point) {
            return Ok(ResponderAction::IntersectFound(point, tip));
        }
        if Some(&point) < points.last() {
            break;
        }
    }
    Ok(ResponderAction::IntersectNotFound(tip))
}

#[derive(Debug)]
pub enum ResponderAction {
    IntersectFound(Point, Tip),
    IntersectNotFound(Tip),
    AwaitReply,
    RollForward(HeaderContent, Tip),
    RollBackward(Point, Tip),
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ResponderResult {
    FindIntersect(Vec<Point>),
    RequestNext,
    Done,
}
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Ord, PartialOrd,
)]
pub enum ResponderState {
    Idle { send_rollback: bool },
    CanAwait { send_rollback: bool },
    MustReply,
    Intersect,
    Done,
}

impl ProtocolState<Responder> for ResponderState {
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
        use ResponderState::*;

        Ok(match (self, input) {
            (Idle { .. }, Message::FindIntersect(points)) => (
                outcome().result(ResponderResult::FindIntersect(points)),
                Intersect,
            ),
            (Idle { send_rollback }, Message::RequestNext) => (
                outcome().result(ResponderResult::RequestNext),
                CanAwait {
                    send_rollback: *send_rollback,
                },
            ),
            (Idle { .. }, Message::Done) => (outcome().result(ResponderResult::Done), Done),
            (this, input) => anyhow::bail!("invalid state: {:?} <- {:?}", this, input),
        })
    }

    fn local(&self, input: Self::Action) -> anyhow::Result<(Outcome<Self::WireMsg, Void>, Self)> {
        use ResponderState::*;

        Ok(match (self, input) {
            (Intersect, ResponderAction::IntersectFound(point, tip)) => (
                outcome().send(Message::IntersectFound(point, tip)),
                Idle {
                    send_rollback: true,
                },
            ),
            (Intersect, ResponderAction::IntersectNotFound(tip)) => (
                outcome().send(Message::IntersectNotFound(tip)),
                Idle {
                    send_rollback: false,
                },
            ),
            (CanAwait { send_rollback }, ResponderAction::AwaitReply) => {
                ensure!(!*send_rollback, "cannot AwaitReply after intersect");
                (outcome().send(Message::AwaitReply), MustReply)
            }
            (CanAwait { send_rollback }, ResponderAction::RollForward(content, tip)) => {
                ensure!(!*send_rollback, "cannot RollForward after intersect");
                (
                    outcome().send(Message::RollForward(content, tip)),
                    Idle {
                        send_rollback: false,
                    },
                )
            }
            (MustReply, ResponderAction::RollForward(content, tip)) => (
                outcome().send(Message::RollForward(content, tip)),
                Idle {
                    send_rollback: false,
                },
            ),
            (CanAwait { .. } | MustReply, ResponderAction::RollBackward(point, tip)) => (
                outcome().send(Message::RollBackward(point, tip)),
                Idle {
                    send_rollback: false,
                },
            ),
            (this, input) => anyhow::bail!("invalid state: {:?} <- {:?}", this, input),
        })
    }
}

#[cfg(test)]
#[expect(clippy::wildcard_enum_match_arm)]
mod tests {
    use super::*;
    use crate::{
        chainsync::initiator::InitiatorState,
        protocol::{ProtoSpec, Role},
    };
    use amaru_kernel::protocol_messages::block_height::BlockHeight;

    #[test]
    fn test_responder_protocol() {
        use Message::{
            AwaitReply, FindIntersect, IntersectFound, IntersectNotFound, RequestNext,
            RollBackward, RollForward,
        };
        use ResponderState::{CanAwait, Done, Idle, Intersect, MustReply};

        // canonical states and messages
        let idle = |send_rollback: bool| Idle { send_rollback };
        let can_await = |send_rollback: bool| CanAwait { send_rollback };
        let find_intersect = || FindIntersect(vec![Point::Origin]);
        let intersect_found =
            || IntersectFound(Point::Origin, Tip::new(Point::Origin, BlockHeight::new(0)));
        let intersect_not_found =
            || IntersectNotFound(Tip::new(Point::Origin, BlockHeight::new(0)));
        let roll_forward = || {
            RollForward(
                HeaderContent {
                    variant: 6,
                    byron_prefix: None,
                    cbor: vec![],
                },
                Tip::new(Point::Origin, BlockHeight::new(0)),
            )
        };
        let roll_backward =
            || RollBackward(Point::Origin, Tip::new(Point::Origin, BlockHeight::new(0)));

        let mut spec = ProtoSpec::default();
        spec.init(idle(false), find_intersect(), Intersect);
        spec.init(idle(true), find_intersect(), Intersect);
        spec.init(idle(false), RequestNext, can_await(false));
        spec.init(idle(true), RequestNext, can_await(true));
        spec.init(idle(false), Message::Done, Done);
        spec.init(idle(true), Message::Done, Done);
        spec.resp(Intersect, intersect_found(), idle(true));
        spec.resp(Intersect, intersect_not_found(), idle(false));
        spec.resp(can_await(false), AwaitReply, MustReply);
        spec.resp(can_await(false), roll_forward(), idle(false));
        spec.resp(can_await(false), roll_backward(), idle(false));
        spec.resp(can_await(true), roll_backward(), idle(false));
        spec.resp(MustReply, roll_forward(), idle(false));
        spec.resp(MustReply, roll_backward(), idle(false));

        spec.check(
            idle(false),
            Role::Responder,
            |msg| match msg {
                AwaitReply => Some(ResponderAction::AwaitReply),
                RollForward(header_content, tip) => {
                    Some(ResponderAction::RollForward(header_content.clone(), *tip))
                }
                RollBackward(point, tip) => Some(ResponderAction::RollBackward(*point, *tip)),
                IntersectFound(point, tip) => Some(ResponderAction::IntersectFound(*point, *tip)),
                IntersectNotFound(tip) => Some(ResponderAction::IntersectNotFound(*tip)),
                _ => None,
            },
            |msg| msg.clone(),
        );

        spec.assert_refines(
            &super::super::initiator::tests::spec(),
            |state| match state {
                Idle { .. } => todo!(),
                CanAwait { .. } => InitiatorState::CanAwait,
                MustReply => InitiatorState::MustReply,
                Intersect => InitiatorState::Intersect,
                Done => InitiatorState::Done,
            },
        );
    }
}
