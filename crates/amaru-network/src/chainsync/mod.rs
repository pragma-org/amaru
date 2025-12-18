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
};
use amaru_kernel::{Point, peer::Peer, protocol_messages::tip::Tip};
use pure_stage::{Effects, StageRef, TryInStage};

mod messages;

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
    state: State,
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
            state: State::default(),
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
            .initiator_step(Input::Local(InitiatorAction::RequestNext)),
        InitiatorMessage::Done => initiator
            .state
            .initiator_step(Input::Local(InitiatorAction::Done)),
        InitiatorMessage::FromNetwork(HandlerMessage::Registered(_)) => initiator
            .state
            .initiator_step(Input::Local(InitiatorAction::Intersect(vec![]))),
        InitiatorMessage::FromNetwork(HandlerMessage::FromNetwork(msg)) => {
            let msg: Message = minicbor::decode(&msg.into_inner())
                .or_terminate(&eff, async |err| {
                    tracing::error!(%err, "failed to decode message from network");
                })
                .await;
            initiator.state.initiator_step(Input::Remote(msg))
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
    if let Some(result) = outcome.done {
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

#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Responder {
    upstream: Tip,
    state: State,
    peer: Peer,
    conn_id: ConnectionId,
    muxer: StageRef<MuxMessage>,
}

impl Responder {
    pub fn new(
        upstream: Tip,
        peer: Peer,
        conn_id: ConnectionId,
        muxer: StageRef<MuxMessage>,
    ) -> Self {
        Self {
            upstream,
            state: State::default(),
            peer,
            conn_id,
            muxer,
        }
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ResponderMessage {
    NewTip(Tip),
    FromNetwork(HandlerMessage),
}

pub async fn responder(
    mut responder: Responder,
    msg: ResponderMessage,
    eff: Effects<ResponderMessage>,
) -> Responder {
    let (outcome, state) = match msg {
        // FIXME send RollForward if the tip advances
        ResponderMessage::NewTip(tip) => {
            responder.upstream = tip;
            Ok((
                outcome().result(ResponderResult::GotNewTip),
                responder.state,
            ))
        }
        ResponderMessage::FromNetwork(HandlerMessage::Registered(_)) => {
            Ok((outcome(), responder.state))
        }
        ResponderMessage::FromNetwork(HandlerMessage::FromNetwork(msg)) => {
            let msg: Message = minicbor::decode(&msg.into_inner())
                .or_terminate(&eff, async |err| {
                    tracing::error!(%err, "failed to decode message from network");
                })
                .await;
            responder.state.responder_step(Input::Remote(msg))
        }
    }
    .or_terminate(&eff, async |err| {
        tracing::error!(%err, "failed to step initiator");
    })
    .await;

    responder.state = state;

    if let Some(msg) = outcome.send {
        let msg = NonEmptyBytes::encode(&msg);
        eff.call(&responder.muxer, NETWORK_SEND_TIMEOUT, move |cr| {
            MuxMessage::Send(PROTO_N2N_CHAIN_SYNC.erase(), msg, cr)
        })
        .await;
    }
    if let Some(result) = outcome.done {
        match &result {
            ResponderResult::GotNewTip => todo!(),
            ResponderResult::FindIntersect(points) => todo!(),
            ResponderResult::RequestNext => todo!(),
        }
    }

    responder
}

#[derive(Debug)]
enum ResponderAction {
    IntersectFound(Point, Tip),
    IntersectNotFound(Tip),
    AwaitReply,
    RollForward(HeaderContent, Tip),
    RollBackward(Point, Tip),
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ResponderResult {
    GotNewTip,
    FindIntersect(Vec<Point>),
    RequestNext,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum State {
    #[default]
    Idle,
    CanAwait,
    MustReply,
    Intersect,
    Done,
}

impl State {
    fn initiator_step(
        self,
        input: Input<InitiatorAction, Message>,
    ) -> anyhow::Result<(Outcome<Message, InitiatorResult>, Self)> {
        Ok(match (self, input) {
            (State::Idle, Input::Local(InitiatorAction::Intersect(points))) => (
                outcome().send(Message::FindIntersect(points)),
                State::Intersect,
            ),
            (State::Intersect, Input::Remote(Message::IntersectFound(point, tip))) => (
                outcome()
                    .send(Message::RequestNext)
                    .result(InitiatorResult::IntersectFound(point, tip)),
                State::CanAwait,
            ),
            (State::Intersect, Input::Remote(Message::IntersectNotFound(tip))) => (
                outcome().result(InitiatorResult::IntersectNotFound(tip)),
                State::Idle,
            ),
            (State::Idle, Input::Local(InitiatorAction::RequestNext)) => {
                (outcome().send(Message::RequestNext), State::CanAwait)
            }
            (State::CanAwait, Input::Remote(Message::AwaitReply)) => (outcome(), State::MustReply),
            (
                State::CanAwait | State::MustReply,
                Input::Remote(Message::RollForward(content, tip)),
            ) => (
                outcome().result(InitiatorResult::RollForward(content, tip)),
                State::Idle,
            ),
            (
                State::CanAwait | State::MustReply,
                Input::Remote(Message::RollBackward(point, tip)),
            ) => (
                outcome().result(InitiatorResult::RollBackward(point, tip)),
                State::Idle,
            ),
            (State::Intersect, Input::Local(InitiatorAction::Done)) => {
                (outcome().send(Message::Done), State::Done)
            }
            (this, input) => anyhow::bail!("invalid state: {:?} <- {:?}", this, input),
        })
    }

    fn responder_step(
        self,
        input: Input<ResponderAction, Message>,
    ) -> anyhow::Result<(Outcome<Message, ResponderResult>, Self)> {
        Ok(match (self, input) {
            (State::Idle, Input::Remote(Message::FindIntersect(points))) => (
                outcome().result(ResponderResult::FindIntersect(points)),
                State::Intersect,
            ),
            (State::Intersect, Input::Local(ResponderAction::IntersectFound(point, tip))) => (
                outcome().send(Message::IntersectFound(point, tip)),
                State::Idle,
            ),
            (State::Intersect, Input::Local(ResponderAction::IntersectNotFound(tip))) => {
                (outcome().send(Message::IntersectNotFound(tip)), State::Idle)
            }
            (State::Idle, Input::Remote(Message::RequestNext)) => (
                outcome().result(ResponderResult::RequestNext),
                State::CanAwait,
            ),
            (State::CanAwait, Input::Local(ResponderAction::AwaitReply)) => {
                (outcome().send(Message::AwaitReply), State::MustReply)
            }
            (
                State::CanAwait | State::MustReply,
                Input::Local(ResponderAction::RollForward(content, tip)),
            ) => (
                outcome().send(Message::RollForward(content, tip)),
                State::Idle,
            ),
            (
                State::CanAwait | State::MustReply,
                Input::Local(ResponderAction::RollBackward(point, tip)),
            ) => (
                outcome().send(Message::RollBackward(point, tip)),
                State::Idle,
            ),
            (this, input) => anyhow::bail!("invalid state: {:?} <- {:?}", this, input),
        })
    }
}
