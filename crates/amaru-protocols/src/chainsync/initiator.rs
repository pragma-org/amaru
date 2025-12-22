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
        Inputs, Miniprotocol, Outcome, PROTO_N2N_CHAIN_SYNC, ProtocolState, StageState,
        miniprotocol, outcome,
    },
    store_effects::Store,
};
use amaru_kernel::{BlockHeader, Point, peer::Peer, protocol_messages::tip::Tip};
use amaru_ouroboros::{ConnectionId, ReadOnlyChainStore};
use pure_stage::{Effects, StageRef};

pub fn initiator() -> Miniprotocol<InitiatorState, Initiator> {
    miniprotocol(PROTO_N2N_CHAIN_SYNC.erase())
}

/// Message sent to the handler from the consensus pipeline
#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum InitiatorMessage {
    RequestNext,
    Done,
}

/// Message sent from the handler to the consensus pipeline
#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ChainSyncInitiatorMsg {
    pub peer: Peer,
    pub conn_id: ConnectionId,
    pub handler: StageRef<InitiatorMessage>,
    pub msg: InitiatorResult,
}

#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Initiator {
    upstream: Option<Tip>,
    peer: Peer,
    conn_id: ConnectionId,
    muxer: StageRef<MuxMessage>,
    pipeline: StageRef<ChainSyncInitiatorMsg>,
    me: StageRef<InitiatorMessage>,
}

impl Initiator {
    pub fn new(
        peer: Peer,
        conn_id: ConnectionId,
        muxer: StageRef<MuxMessage>,
        pipeline: StageRef<ChainSyncInitiatorMsg>,
    ) -> (InitiatorState, Self) {
        (
            InitiatorState::Idle,
            Self {
                upstream: None,
                peer,
                conn_id,
                muxer,
                pipeline,
                me: StageRef::blackhole(),
            },
        )
    }
}

impl AsRef<StageRef<MuxMessage>> for Initiator {
    fn as_ref(&self) -> &StageRef<MuxMessage> {
        &self.muxer
    }
}

impl StageState<InitiatorState> for Initiator {
    type LocalIn = InitiatorMessage;

    async fn local(
        self,
        proto: &InitiatorState,
        input: Self::LocalIn,
        _eff: &Effects<Inputs<Self::LocalIn>>,
    ) -> anyhow::Result<(Self, Option<<InitiatorState as ProtocolState>::Action>)> {
        use InitiatorState::*;

        Ok(match (proto, input) {
            (Idle, InitiatorMessage::RequestNext) => (self, Some(InitiatorAction::RequestNext)),
            (Idle, InitiatorMessage::Done) => (self, Some(InitiatorAction::Done)),
            (this, input) => anyhow::bail!("invalid state: {:?} <- {:?}", this, input),
        })
    }

    async fn network(
        mut self,
        _proto: &InitiatorState,
        input: <InitiatorState as ProtocolState>::Out,
        eff: &Effects<Inputs<Self::LocalIn>>,
    ) -> anyhow::Result<(Self, Option<<InitiatorState as ProtocolState>::Action>)> {
        use InitiatorAction::*;
        let action = match &input {
            InitiatorResult::Initialize => {
                self.me = eff
                    .contramap(
                        eff.me(),
                        format!("{}-handler", eff.me().name()),
                        Inputs::Local,
                    )
                    .await;
                Some(Intersect(intersect_points(&Store::new(eff.clone()))))
            }
            InitiatorResult::IntersectFound(_, tip)
            | InitiatorResult::IntersectNotFound(tip)
            | InitiatorResult::RollForward(_, tip)
            | InitiatorResult::RollBackward(_, tip) => {
                self.upstream = Some(*tip);
                None
            }
        };
        eff.send(
            &self.pipeline,
            ChainSyncInitiatorMsg {
                peer: self.peer.clone(),
                conn_id: self.conn_id,
                handler: self.me.clone(),
                msg: input,
            },
        )
        .await;
        Ok((self, action))
    }
}

fn intersect_points(store: &dyn ReadOnlyChainStore<BlockHeader>) -> Vec<Point> {
    let mut spacing = 1;
    let mut points = Vec::new();
    let best = store.get_best_chain_hash();
    #[expect(clippy::expect_used)]
    let best = store.load_header(&best).expect("best chain hash is valid");
    let mut last = best.tip().point();
    for (index, header) in store.ancestors(best).enumerate() {
        last = header.tip().point();
        if index == spacing {
            points.push(last);
            spacing *= 2;
        }
    }
    points.push(last);
    points
}

#[derive(Debug)]
pub enum InitiatorAction {
    Intersect(Vec<Point>),
    RequestNext,
    Done,
}

#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum InitiatorResult {
    Initialize,
    IntersectFound(Point, Tip),
    IntersectNotFound(Tip),
    RollForward(HeaderContent, Tip),
    RollBackward(Point, Tip),
}
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub enum InitiatorState {
    Idle,
    CanAwait,
    MustReply,
    Intersect,
    Done,
}

impl ProtocolState for InitiatorState {
    type WireMsg = Message;
    type Action = InitiatorAction;
    type Out = InitiatorResult;

    fn init(&self) -> anyhow::Result<(Outcome<Self::WireMsg, Self::Out>, Self)> {
        Ok((outcome().result(InitiatorResult::Initialize), *self))
    }

    fn network(
        &self,
        input: Self::WireMsg,
    ) -> anyhow::Result<(Outcome<Self::WireMsg, Self::Out>, Self)> {
        use InitiatorState::*;

        Ok(match (self, input) {
            (Intersect, Message::IntersectFound(point, tip)) => (
                outcome()
                    .send(Message::RequestNext)
                    .result(InitiatorResult::IntersectFound(point, tip)),
                CanAwait,
            ),
            (Intersect, Message::IntersectNotFound(tip)) => (
                outcome().result(InitiatorResult::IntersectNotFound(tip)),
                Idle,
            ),
            (CanAwait, Message::AwaitReply) => (outcome(), MustReply),
            (CanAwait | MustReply, Message::RollForward(content, tip)) => (
                outcome().result(InitiatorResult::RollForward(content, tip)),
                Idle,
            ),
            (CanAwait | MustReply, Message::RollBackward(point, tip)) => (
                outcome().result(InitiatorResult::RollBackward(point, tip)),
                Idle,
            ),
            (this, input) => anyhow::bail!("invalid state: {:?} <- {:?}", this, input),
        })
    }

    fn local(&self, input: Self::Action) -> anyhow::Result<(Option<Self::WireMsg>, Self)> {
        use InitiatorState::*;

        Ok(match (self, input) {
            (Idle, InitiatorAction::Intersect(points)) => {
                (Some(Message::FindIntersect(points)), Intersect)
            }
            (Idle, InitiatorAction::RequestNext) => (Some(Message::RequestNext), CanAwait),
            (Idle, InitiatorAction::Done) => (Some(Message::Done), Done),
            (this, input) => anyhow::bail!("invalid state: {:?} <- {:?}", this, input),
        })
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::protocol::{ProtoSpec, Role};
    use InitiatorState::*;
    use Message::*;
    use amaru_kernel::protocol_messages::block_height::BlockHeight;

    pub fn spec() -> ProtoSpec<InitiatorState, Message> {
        // canonical states and messages
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
        spec.i(Idle, find_intersect(), Intersect);
        spec.i(Idle, Message::Done, InitiatorState::Done);
        spec.i(Idle, Message::RequestNext, CanAwait);
        spec.r(Intersect, intersect_found(), Idle);
        spec.r(Intersect, intersect_not_found(), Idle);
        spec.r(CanAwait, AwaitReply, MustReply);
        spec.r(CanAwait, roll_forward(), Idle);
        spec.r(CanAwait, roll_backward(), Idle);
        spec.r(MustReply, roll_forward(), Idle);
        spec.r(MustReply, roll_backward(), Idle);
        spec
    }

    #[test]
    fn test_initiator_protocol() {
        spec().check(
            Idle,
            Role::Initiator,
            |msg| match msg {
                FindIntersect(points) => Some(InitiatorAction::Intersect(points.clone())),
                RequestNext => Some(InitiatorAction::RequestNext),
                Message::Done => Some(InitiatorAction::Done),
                _ => None,
            },
            |msg| msg.clone(),
        );
    }
}
