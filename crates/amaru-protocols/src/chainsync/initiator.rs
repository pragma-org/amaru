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
        Initiator, Inputs, Miniprotocol, Outcome, PROTO_N2N_CHAIN_SYNC, ProtocolState, StageState,
        miniprotocol, outcome,
    },
    store_effects::Store,
};
use amaru_kernel::{BlockHeader, ORIGIN_HASH, Peer, Point, Tip};
use amaru_ouroboros::{ConnectionId, ReadOnlyChainStore};
use pure_stage::{DeserializerGuards, Effects, StageRef, Void};
use tracing::instrument;

pub fn register_deserializers() -> DeserializerGuards {
    vec![
        pure_stage::register_data_deserializer::<InitiatorMessage>().boxed(),
        pure_stage::register_data_deserializer::<ChainSyncInitiatorMsg>().boxed(),
        pure_stage::register_data_deserializer::<ChainSyncInitiator>().boxed(),
    ]
}

pub fn initiator() -> Miniprotocol<InitiatorState, ChainSyncInitiator, Initiator> {
    miniprotocol(PROTO_N2N_CHAIN_SYNC)
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

impl ChainSyncInitiatorMsg {
    pub fn message_type(&self) -> &str {
        match self.msg {
            InitiatorResult::Initialize => "Initialize",
            InitiatorResult::IntersectFound(_, _) => "IntersectFound",
            InitiatorResult::IntersectNotFound(_) => "IntersectNotFound",
            InitiatorResult::RollForward(_, _) => "RollForward",
            InitiatorResult::RollBackward(_, _) => "RollBackward",
        }
    }
}

#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ChainSyncInitiator {
    upstream: Option<Tip>,
    peer: Peer,
    conn_id: ConnectionId,
    muxer: StageRef<MuxMessage>,
    pipeline: StageRef<ChainSyncInitiatorMsg>,
    me: StageRef<InitiatorMessage>,
}

impl ChainSyncInitiator {
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

impl StageState<InitiatorState, Initiator> for ChainSyncInitiator {
    type LocalIn = InitiatorMessage;

    async fn local(
        self,
        proto: &InitiatorState,
        input: Self::LocalIn,
        _eff: &Effects<Inputs<Self::LocalIn>>,
    ) -> anyhow::Result<(
        Option<<InitiatorState as ProtocolState<Initiator>>::Action>,
        Self,
    )> {
        use InitiatorState::*;

        Ok(match (proto, input) {
            (Idle, InitiatorMessage::RequestNext) => (Some(InitiatorAction::RequestNext), self),
            (CanAwait(_) | MustReply(_), InitiatorMessage::RequestNext) => {
                (Some(InitiatorAction::RequestNext), self)
            }
            (Idle, InitiatorMessage::Done) => (Some(InitiatorAction::Done), self),
            (this, input) => anyhow::bail!("invalid state: {:?} <- {:?}", this, input),
        })
    }

    async fn network(
        mut self,
        _proto: &InitiatorState,
        input: <InitiatorState as ProtocolState<Initiator>>::Out,
        eff: &Effects<Inputs<Self::LocalIn>>,
    ) -> anyhow::Result<(
        Option<<InitiatorState as ProtocolState<Initiator>>::Action>,
        Self,
    )> {
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
        Ok((action, self))
    }

    fn muxer(&self) -> &StageRef<MuxMessage> {
        &self.muxer
    }
}

fn intersect_points(store: &dyn ReadOnlyChainStore<BlockHeader>) -> Vec<Point> {
    let mut spacing = 1;
    let mut points = Vec::new();
    let best = store.get_best_chain_hash();
    if best == ORIGIN_HASH {
        return vec![Point::Origin];
    }
    #[expect(clippy::expect_used)]
    let best = store.load_header(&best).expect("best chain hash is valid");
    let best_point = best.tip().point();
    points.push(best_point);

    let mut last = best_point;
    for (index, header) in store.ancestors(best).enumerate() {
        last = header.tip().point();
        if index + 1 == spacing {
            points.push(last);
            spacing *= 2;
        }
    }
    if points.last() != Some(&last) {
        points.push(last);
    }
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
    CanAwait(u8),
    MustReply(u8),
    Intersect,
    Done,
}

impl ProtocolState<Initiator> for InitiatorState {
    type WireMsg = Message;
    type Action = InitiatorAction;
    type Out = InitiatorResult;
    type Error = Void;

    fn init(&self) -> anyhow::Result<(Outcome<Self::WireMsg, Self::Out, Self::Error>, Self)> {
        Ok((outcome().result(InitiatorResult::Initialize), *self))
    }

    #[instrument(name = "chainsync.initiator", skip_all, fields(message_type = input.message_type()))]
    fn network(
        &self,
        input: Self::WireMsg,
    ) -> anyhow::Result<(Outcome<Self::WireMsg, Self::Out, Self::Error>, Self)> {
        use InitiatorState::*;

        Ok(match (self, input) {
            (Intersect, Message::IntersectFound(point, tip)) => (
                // only for this first time do we sent two requests
                // this initiates the desired pipelining behaviour
                outcome()
                    .send(Message::RequestNext(2))
                    .want_next()
                    .result(InitiatorResult::IntersectFound(point, tip)),
                CanAwait(1),
            ),
            (Intersect, Message::IntersectNotFound(tip)) => (
                outcome().result(InitiatorResult::IntersectNotFound(tip)),
                Idle,
            ),
            (CanAwait(n), Message::AwaitReply) => (outcome().want_next(), MustReply(*n)),
            (CanAwait(n) | MustReply(n), Message::RollForward(content, tip)) => (
                outcome().result(InitiatorResult::RollForward(content, tip)),
                if *n == 0 { Idle } else { CanAwait(*n - 1) },
            ),
            (CanAwait(n) | MustReply(n), Message::RollBackward(point, tip)) => (
                outcome().result(InitiatorResult::RollBackward(point, tip)),
                if *n == 0 { Idle } else { CanAwait(*n - 1) },
            ),
            (this, input) => anyhow::bail!("invalid state: {:?} <- {:?}", this, input),
        })
    }

    fn local(
        &self,
        input: Self::Action,
    ) -> anyhow::Result<(Outcome<Self::WireMsg, Void, Self::Error>, Self)> {
        use InitiatorState::*;

        Ok(match (self, input) {
            (Idle, InitiatorAction::Intersect(points)) => (
                outcome().send(Message::FindIntersect(points)).want_next(),
                Intersect,
            ),
            (Idle, InitiatorAction::RequestNext) => (
                outcome().send(Message::RequestNext(1)).want_next(),
                CanAwait(0),
            ),
            (CanAwait(n), InitiatorAction::RequestNext) => (
                outcome().send(Message::RequestNext(1)).want_next(),
                CanAwait(*n + 1),
            ),
            (MustReply(n), InitiatorAction::RequestNext) => (
                outcome().send(Message::RequestNext(1)).want_next(),
                MustReply(*n + 1),
            ),
            (Idle, InitiatorAction::Done) => (outcome().send(Message::Done), Done),
            (this, input) => anyhow::bail!("invalid state: {:?} <- {:?}", this, input),
        })
    }
}

#[cfg(test)]
#[expect(clippy::wildcard_enum_match_arm)]
pub mod tests {
    use super::*;
    use crate::protocol::ProtoSpec;
    use InitiatorState::*;
    use Message::*;
    use amaru_kernel::{Hash, HeaderHash, RawBlock, Slot, make_header, size::HEADER};
    use amaru_ouroboros_traits::{Nonces, StoreError};

    pub fn spec() -> ProtoSpec<InitiatorState, Message, Initiator> {
        // canonical states and messages
        let find_intersect = || FindIntersect(vec![Point::Origin]);
        let intersect_found = || IntersectFound(Point::Origin, Tip::origin());
        let intersect_not_found = || IntersectNotFound(Tip::origin());
        let roll_forward = || RollForward(HeaderContent::make_v6(vec![]), Tip::origin());
        let roll_backward = || RollBackward(Point::Origin, Tip::origin());

        let mut spec = ProtoSpec::default();
        spec.init(Idle, find_intersect(), Intersect);
        spec.init(Idle, Message::Done, InitiatorState::Done);
        spec.init(Idle, Message::RequestNext(1), CanAwait(0));
        spec.resp(Intersect, intersect_found(), Idle);
        spec.resp(Intersect, intersect_not_found(), Idle);
        spec.resp(CanAwait(0), AwaitReply, MustReply(0));
        spec.resp(CanAwait(0), roll_forward(), Idle);
        spec.resp(CanAwait(0), roll_backward(), Idle);
        spec.resp(MustReply(0), roll_forward(), Idle);
        spec.resp(MustReply(0), roll_backward(), Idle);
        spec
    }

    #[test]
    #[ignore = "pipelining cannot be tested yet"]
    fn test_initiator_protocol() {
        spec().check(Idle, |msg| match msg {
            FindIntersect(points) => Some(InitiatorAction::Intersect(points.clone())),
            RequestNext(1) => Some(InitiatorAction::RequestNext),
            Message::Done => Some(InitiatorAction::Done),
            _ => None,
        });
    }

    #[test]
    fn test_intersect_points_includes_best_point_and_are_spaced_with_a_factor_2() {
        let store = MockChainStoreForIntersectPoints::default();
        let points = intersect_points(&store);
        let slots = points
            .iter()
            .map(|p| p.slot_or_default().into())
            .collect::<Vec<u64>>();
        // The expected slots contain the best point (100) and the other points are spaced with a factor of 2.
        assert_eq!(slots, vec![100, 99, 98, 96, 92, 84, 68, 36, 0]);
    }

    /// This chain store contains a chain of 100 blocks with slots from 0 to 100 where 100 is the best point.
    #[derive(Debug)]
    struct MockChainStoreForIntersectPoints {
        best_point: Point,
    }

    impl Default for MockChainStoreForIntersectPoints {
        fn default() -> Self {
            Self {
                best_point: Point::Specific(Slot::from(100), Hash::new([100u8; HEADER])),
            }
        }
    }

    #[expect(clippy::todo)]
    impl ReadOnlyChainStore<BlockHeader> for MockChainStoreForIntersectPoints {
        fn get_best_chain_hash(&self) -> HeaderHash {
            self.best_point.hash()
        }

        fn load_header(&self, _hash: &HeaderHash) -> Option<BlockHeader> {
            Some(BlockHeader::new(
                make_header(1, self.best_point.slot_or_default().into(), None),
                self.best_point.hash(),
            ))
        }

        fn ancestors<'a>(&'a self, _from: BlockHeader) -> Box<dyn Iterator<Item = BlockHeader> + 'a>
        where
            BlockHeader: 'a,
        {
            let mut ancestor_block_headers = vec![];
            for slot in 0..100 {
                let header_hash = Hash::new([slot as u8; HEADER]);
                let block_header = BlockHeader::new(make_header(1, slot, None), header_hash);
                ancestor_block_headers.push(block_header);
            }
            ancestor_block_headers.reverse();
            Box::new(ancestor_block_headers.into_iter())
        }

        fn get_children(&self, _hash: &HeaderHash) -> Vec<HeaderHash> {
            todo!()
        }

        fn get_anchor_hash(&self) -> HeaderHash {
            todo!()
        }

        fn load_from_best_chain(&self, _point: &Point) -> Option<HeaderHash> {
            todo!()
        }

        fn next_best_chain(&self, _point: &Point) -> Option<Point> {
            todo!()
        }

        fn load_block(&self, _hash: &HeaderHash) -> Result<Option<RawBlock>, StoreError> {
            todo!()
        }

        fn get_nonces(&self, _header: &HeaderHash) -> Option<Nonces> {
            todo!()
        }

        fn has_header(&self, _hash: &HeaderHash) -> bool {
            todo!()
        }
    }
}
