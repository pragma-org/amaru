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
    bytes::NonEmptyBytes,
    chainsync::messages::{HeaderContent, Message},
    mux::{HandlerMessage, MuxMessage},
    protocol::{Input, NETWORK_SEND_TIMEOUT, Outcome, PROTO_N2N_CHAIN_SYNC, outcome},
    socket::ConnectionId,
    store_effects::Store,
};
use amaru_kernel::{BlockHeader, Point, peer::Peer, protocol_messages::tip::Tip};
use amaru_ouroboros::ReadOnlyChainStore;
use pure_stage::{Effects, StageRef, TryInStage};

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
    state: InitiatorState,
    peer: Peer,
    conn_id: ConnectionId,
    muxer: StageRef<MuxMessage>,
    pipeline: StageRef<ChainSyncInitiatorMsg>,
}

impl Initiator {
    pub fn new(
        peer: Peer,
        conn_id: ConnectionId,
        muxer: StageRef<MuxMessage>,
        pipeline: StageRef<ChainSyncInitiatorMsg>,
    ) -> Self {
        Self {
            upstream: None,
            state: InitiatorState::Idle,
            peer,
            conn_id,
            muxer,
            pipeline,
        }
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum InitiatorMessage {
    RequestNext,
    Done,
    FromNetwork(HandlerMessage),
}

pub async fn initiator(
    mut initiator: Initiator,
    msg: InitiatorMessage,
    eff: Effects<InitiatorMessage>,
) -> Initiator {
    let (outcome, state) = match msg {
        InitiatorMessage::RequestNext => initiator
            .state
            .step(Input::Local(InitiatorAction::RequestNext)),
        InitiatorMessage::Done => initiator.state.step(Input::Local(InitiatorAction::Done)),
        InitiatorMessage::FromNetwork(HandlerMessage::Registered(_)) => {
            initiator
                .state
                .step(Input::Local(InitiatorAction::Intersect(intersect_points(
                    &Store::new(eff.clone()),
                ))))
        }
        InitiatorMessage::FromNetwork(HandlerMessage::FromNetwork(msg)) => {
            let msg: Message = minicbor::decode(&msg.into_inner())
                .or_terminate(&eff, async |err| {
                    tracing::error!(%err, "failed to decode message from network");
                })
                .await;
            initiator.state.step(Input::Remote(msg))
        }
    }
    .or_terminate(&eff, async |err| {
        tracing::error!(%err, "failed to step initiator");
    })
    .await;

    initiator.state = state;

    if let Some(msg) = outcome.send {
        let msg = NonEmptyBytes::encode(&msg);
        eff.call(&initiator.muxer, NETWORK_SEND_TIMEOUT, move |cr| {
            MuxMessage::Send(PROTO_N2N_CHAIN_SYNC.erase(), msg, cr)
        })
        .await;
    }
    if let Some(result) = outcome.result {
        match &result {
            InitiatorResult::IntersectFound(_point, tip) => initiator.upstream = Some(*tip),
            InitiatorResult::IntersectNotFound(tip) => initiator.upstream = Some(*tip),
            InitiatorResult::RollForward(_header_content, tip) => initiator.upstream = Some(*tip),
            InitiatorResult::RollBackward(_point, tip) => initiator.upstream = Some(*tip),
        }
        let msg = ChainSyncInitiatorMsg {
            peer: initiator.peer.clone(),
            conn_id: initiator.conn_id,
            handler: eff.me(),
            msg: result,
        };
        eff.send(&initiator.pipeline, msg).await;
    }

    initiator
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
enum InitiatorAction {
    Intersect(Vec<Point>),
    RequestNext,
    Done,
}

#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum InitiatorResult {
    IntersectFound(Point, Tip),
    IntersectNotFound(Tip),
    RollForward(HeaderContent, Tip),
    RollBackward(Point, Tip),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum InitiatorState {
    Idle,
    CanAwait,
    MustReply,
    Intersect,
    Done,
}

impl InitiatorState {
    fn step(
        self,
        input: Input<InitiatorAction, Message>,
    ) -> anyhow::Result<(Outcome<Message, InitiatorResult>, Self)> {
        use InitiatorState::*;

        Ok(match (self, input) {
            (Idle, Input::Local(InitiatorAction::Intersect(points))) => {
                (outcome().send(Message::FindIntersect(points)), Intersect)
            }
            (Intersect, Input::Remote(Message::IntersectFound(point, tip))) => (
                outcome()
                    .send(Message::RequestNext)
                    .result(InitiatorResult::IntersectFound(point, tip)),
                CanAwait,
            ),
            (Intersect, Input::Remote(Message::IntersectNotFound(tip))) => (
                outcome().result(InitiatorResult::IntersectNotFound(tip)),
                Idle,
            ),
            (Idle, Input::Local(InitiatorAction::RequestNext)) => {
                (outcome().send(Message::RequestNext), CanAwait)
            }
            (CanAwait, Input::Remote(Message::AwaitReply)) => (outcome(), MustReply),
            (CanAwait | MustReply, Input::Remote(Message::RollForward(content, tip))) => (
                outcome().result(InitiatorResult::RollForward(content, tip)),
                Idle,
            ),
            (CanAwait | MustReply, Input::Remote(Message::RollBackward(point, tip))) => (
                outcome().result(InitiatorResult::RollBackward(point, tip)),
                Idle,
            ),
            (Intersect, Input::Local(InitiatorAction::Done)) => {
                (outcome().send(Message::Done), Done)
            }
            (this, input) => anyhow::bail!("invalid state: {:?} <- {:?}", this, input),
        })
    }
}
