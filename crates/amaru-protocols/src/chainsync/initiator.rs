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

use amaru_kernel::{Peer, Point, Tip};
use amaru_observability::trace_span;
use amaru_ouroboros::ConnectionId;
use pure_stage::{DeserializerGuards, Effects, SendData, StageRef, Void};
use tracing::Instrument;

use crate::{
    chainsync::messages::{HeaderContent, Message},
    mux::MuxMessage,
    protocol::{
        Initiator, Inputs, Miniprotocol, Outcome, PROTO_N2N_CHAIN_SYNC, ProtocolState, StageState, miniprotocol,
        outcome,
    },
    store_effects::Store,
};

pub fn register_deserializers() -> DeserializerGuards {
    vec![
        pure_stage::register_data_deserializer::<InitiatorMessage>().boxed(),
        pure_stage::register_data_deserializer::<(InitiatorState, ChainSyncInitiator)>().boxed(),
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

impl InitiatorMessage {
    pub fn message_type(&self) -> &str {
        match self {
            InitiatorMessage::RequestNext => "RequestNext",
            InitiatorMessage::Done => "Done",
        }
    }
}

/// Message sent from the handler to the consensus pipeline
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
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
        (InitiatorState::Idle, Self { upstream: None, peer, conn_id, muxer, pipeline, me: StageRef::blackhole() })
    }
}

impl StageState<InitiatorState, Initiator> for ChainSyncInitiator {
    type LocalIn = InitiatorMessage;

    async fn local(
        self,
        proto: &InitiatorState,
        input: Self::LocalIn,
        _eff: &Effects<Inputs<Self::LocalIn>>,
    ) -> anyhow::Result<(Option<<InitiatorState as ProtocolState<Initiator>>::Action>, Self)> {
        use InitiatorState::*;

        Ok(match (proto, input) {
            (Idle, InitiatorMessage::RequestNext) => (Some(InitiatorAction::RequestNext), self),
            (CanAwait(_) | MustReply(_), InitiatorMessage::RequestNext) => (Some(InitiatorAction::RequestNext), self),
            (Idle, InitiatorMessage::Done) => (Some(InitiatorAction::Done), self),
            (this, input) => anyhow::bail!("invalid state: {:?} <- {:?}", this, input),
        })
    }

    async fn network(
        mut self,
        _proto: &InitiatorState,
        input: <InitiatorState as ProtocolState<Initiator>>::Out,
        eff: &Effects<Inputs<Self::LocalIn>>,
    ) -> anyhow::Result<(Option<<InitiatorState as ProtocolState<Initiator>>::Action>, Self)> {
        let message_type = input.message_type().to_string();

        async move {
            use InitiatorAction::*;
            let action = match &input {
                InitiatorResult::Initialize => {
                    self.me = eff.contramap(eff.me(), format!("{}-handler", eff.me().name()), Inputs::Local).await;
                    Some(Intersect(intersect_points(&Store::new(eff.clone())).await))
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
        .instrument(trace_span!(
            amaru_observability::amaru::protocols::chainsync::initiator::CHAINSYNC_INITIATOR_STAGE,
            message_type = message_type
        ))
        .await
    }

    fn muxer(&self) -> &StageRef<MuxMessage> {
        &self.muxer
    }
}

#[tracing::instrument(level = "debug", skip_all)]
async fn intersect_points<T: SendData + Sync + 'static>(store: &Store<T>) -> Vec<Point> {
    let points = store.sample_ancestor_points().await;
    tracing::info!(?points, "intersect points");
    points
}

#[derive(Debug)]
pub enum InitiatorAction {
    Intersect(Vec<Point>),
    RequestNext,
    Done,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum InitiatorResult {
    Initialize,
    IntersectFound(Point, Tip),
    IntersectNotFound(Tip),
    RollForward(HeaderContent, Tip),
    RollBackward(Point, Tip),
}

impl InitiatorResult {
    pub fn message_type(&self) -> &str {
        match self {
            InitiatorResult::Initialize => "Initialize",
            InitiatorResult::IntersectFound(_, _) => "IntersectFound",
            InitiatorResult::IntersectNotFound(_) => "IntersectNotFound",
            InitiatorResult::RollForward(_, _) => "RollForward",
            InitiatorResult::RollBackward(_, _) => "RollBackward",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
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

    fn network(&self, input: Self::WireMsg) -> anyhow::Result<(Outcome<Self::WireMsg, Self::Out, Self::Error>, Self)> {
        let _span = trace_span!(
            amaru_observability::amaru::protocols::chainsync::initiator::CHAINSYNC_INITIATOR_PROTOCOL,
            message_type = input.message_type().to_string()
        );
        let _guard = _span.enter();
        use InitiatorState::*;

        Ok(match (self, input) {
            (Intersect, Message::IntersectFound(point, tip)) => (
                // only for this first time do we sent two requests
                // this initiates the desired pipelining behaviour
                outcome()
                    .send(Message::RequestNext(10))
                    .want_next()
                    .result(InitiatorResult::IntersectFound(point, tip)),
                CanAwait(1),
            ),
            (Intersect, Message::IntersectNotFound(tip)) => {
                (outcome().result(InitiatorResult::IntersectNotFound(tip)), Idle)
            }
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

    fn local(&self, input: Self::Action) -> anyhow::Result<(Outcome<Self::WireMsg, Void, Self::Error>, Self)> {
        use InitiatorState::*;

        Ok(match (self, input) {
            (Idle, InitiatorAction::Intersect(points)) => {
                (outcome().send(Message::FindIntersect(points)).want_next(), Intersect)
            }
            (Idle, InitiatorAction::RequestNext) => (outcome().send(Message::RequestNext(1)).want_next(), CanAwait(0)),
            (CanAwait(n), InitiatorAction::RequestNext) => {
                (outcome().send(Message::RequestNext(1)).want_next(), CanAwait(*n + 1))
            }
            (MustReply(n), InitiatorAction::RequestNext) => {
                (outcome().send(Message::RequestNext(1)).want_next(), MustReply(*n + 1))
            }
            (Idle, InitiatorAction::Done) => (outcome().send(Message::Done), Done),
            (this, input) => anyhow::bail!("invalid state: {:?} <- {:?}", this, input),
        })
    }
}

#[cfg(test)]
#[expect(clippy::wildcard_enum_match_arm)]
pub mod tests {
    use std::sync::Arc;

    use InitiatorState::*;
    use Message::*;
    use amaru_kernel::{BlockHeader, EraName, IsHeader, make_header};
    use amaru_ouroboros_traits::{ChainStore, in_memory_consensus_store::InMemConsensusStore};
    use pure_stage::{Effect, StageGraph, trace_buffer::TraceBuffer};
    use tokio::runtime::Builder;

    use super::*;
    use crate::{protocol::ProtoSpec, store_effects::ResourceHeaderStore};

    pub fn spec() -> ProtoSpec<InitiatorState, Message, Initiator> {
        // canonical states and messages
        let find_intersect = || FindIntersect(vec![Point::Origin]);
        let intersect_found = || IntersectFound(Point::Origin, Tip::origin());
        let intersect_not_found = || IntersectNotFound(Tip::origin());
        let roll_forward = || RollForward(HeaderContent::with_bytes(vec![], EraName::Conway), Tip::origin());
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

    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    enum IntersectPointsTestMsg {
        Run,
    }

    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    struct IntersectPointsTestState {
        output: StageRef<Vec<Point>>,
    }

    async fn intersect_points_test_stage(
        state: IntersectPointsTestState,
        msg: IntersectPointsTestMsg,
        eff: Effects<IntersectPointsTestMsg>,
    ) -> IntersectPointsTestState {
        match msg {
            IntersectPointsTestMsg::Run => {
                let points = intersect_points(&Store::new(eff.clone())).await;
                eff.send(&state.output, points).await;
                state
            }
        }
    }

    #[test]
    fn test_intersect_points_includes_best_point_and_are_spaced_with_a_factor_2() {
        let runtime = Builder::new_current_thread().enable_all().build().unwrap();
        let store = Arc::new(InMemConsensusStore::new());
        let mut parent = None;
        let mut chain = Vec::new();
        for slot in 0..=100 {
            let header = BlockHeader::from(make_header(slot + 1, slot, parent));
            store.store_header(&header).unwrap();
            store.roll_forward_chain(&header.point()).unwrap();
            parent = Some(header.hash());
            chain.push(header);
        }
        let best = chain.last().unwrap();
        store.set_best_chain_hash(&best.hash()).unwrap();

        let trace_buffer = TraceBuffer::new_shared(100, 1_000_000);
        let _guard = TraceBuffer::drop_guard(&trace_buffer);
        let mut network = pure_stage::simulation::SimulationBuilder::default().with_trace_buffer(trace_buffer);
        network.resources().put::<ResourceHeaderStore>(store as Arc<dyn ChainStore<BlockHeader>>);

        let (output, mut rx) = network.output::<Vec<Point>>("intersect_points_output", 1);
        let stage = network.stage("intersect_points_test", intersect_points_test_stage);
        let stage = network.wire_up(stage, IntersectPointsTestState { output: output.clone() });
        network.preload(&stage, [IntersectPointsTestMsg::Run]).unwrap();

        let output_name = output.name().clone();
        let mut running = network.run();
        running.breakpoint(
            "output",
            move |eff| matches!(eff, Effect::External { at_stage, .. } if at_stage == &output_name),
        );

        let effect = running.run_until_blocked_incl_effects(runtime.handle()).assert_breakpoint("output");
        running.handle_effect(effect);
        runtime.block_on(running.await_external_effect()).unwrap();

        let points = rx.try_next().unwrap();
        let slots = points.iter().map(|point| point.slot_or_default().into()).collect::<Vec<u64>>();
        assert_eq!(slots, vec![100, 99, 98, 96, 92, 84, 68, 36, 0]);
    }
}
