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
use amaru_kernel::{
    BlockHeader, IsHeader, Point, peer::Peer, protocol_messages::tip::Tip, to_cbor,
};
use amaru_ouroboros::ReadOnlyChainStore;
use pure_stage::{Effects, StageRef, TryInStage};
use std::cmp::Reverse;

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
        InitiatorMessage::FromNetwork(HandlerMessage::Registered(_)) => initiator
            .state
            .step(Input::Local(InitiatorAction::Intersect(vec![/* FIXME */]))),
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
    state: ResponderState,
    peer: Peer,
    pointer: Point,
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
            state: ResponderState::Idle {
                send_rollback: false,
            },
            peer,
            pointer: Point::Origin,
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
    enum Msg {
        NewTip(Tip),
        Registered,
        Bytes(NonEmptyBytes),
        Action(ResponderAction),
    }
    impl From<ResponderMessage> for Msg {
        fn from(msg: ResponderMessage) -> Self {
            match msg {
                ResponderMessage::NewTip(tip) => Msg::NewTip(tip),
                ResponderMessage::FromNetwork(HandlerMessage::Registered(_)) => Msg::Registered,
                ResponderMessage::FromNetwork(HandlerMessage::FromNetwork(msg)) => Msg::Bytes(msg),
            }
        }
    }

    let mut msg = Msg::from(msg);

    loop {
        let (outcome, state) = match msg {
            Msg::NewTip(tip) => {
                responder.upstream = tip;
                Ok((
                    outcome().result(ResponderResult::GotNewTip),
                    responder.state,
                ))
            }
            Msg::Registered => Ok((outcome(), responder.state)),
            Msg::Bytes(msg) => {
                let msg: Message = minicbor::decode(&msg.into_inner())
                    .or_terminate(&eff, async |err| {
                        tracing::error!(%err, "failed to decode message from network");
                    })
                    .await;
                responder.state.step(Input::Remote(msg))
            }
            Msg::Action(action) => responder.state.step(Input::Local(action)),
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
        if let Some(result) = outcome.result {
            let action = match result {
                ResponderResult::FindIntersect(points) => Some(
                    intersect(points, &Store::new(eff.clone()), responder.upstream)
                        .or_terminate(&eff, async |err| {
                            tracing::error!(%err, "failed to find intersect");
                        })
                        .await,
                ),
                ResponderResult::GotNewTip | ResponderResult::RequestNext => {
                    next_header(
                        responder.state,
                        responder.pointer,
                        &Store::new(eff.clone()),
                        responder.upstream,
                    )
                    .or_terminate(&eff, async |err| {
                        tracing::error!(%err, "failed to get next header");
                    })
                    .await
                }
            };
            if let Some(action) = action {
                msg = Msg::Action(action);
                continue;
            }
        }
        break;
    }

    responder
}

fn next_header(
    state: ResponderState,
    pointer: Point,
    store: &dyn ReadOnlyChainStore<BlockHeader>,
    tip: Tip,
) -> anyhow::Result<Option<ResponderAction>> {
    let (ResponderState::CanAwait { send_rollback } | ResponderState::MustReply { send_rollback }) =
        state
    else {
        return Ok(None);
    };
    if send_rollback {
        return Ok(Some(ResponderAction::RollBackward(pointer, tip)));
    }
    if pointer == tip.point() {
        return Ok((matches!(state, ResponderState::CanAwait { .. }))
            .then_some(ResponderAction::AwaitReply));
    }
    if store.load_from_best_chain(&pointer).is_none() {
        // client is on a different fork, we need to roll backward
        let header = store
            .load_header(&pointer.hash())
            .ok_or_else(|| anyhow::anyhow!("tip not found"))?;
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
                    variant: 6,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ResponderState {
    Idle { send_rollback: bool },
    CanAwait { send_rollback: bool },
    MustReply { send_rollback: bool },
    Intersect,
    Done,
}

impl ResponderState {
    fn step(
        self,
        input: Input<ResponderAction, Message>,
    ) -> anyhow::Result<(Outcome<Message, ResponderResult>, Self)> {
        use ResponderState::*;

        Ok(match (self, input) {
            (Idle { .. }, Input::Remote(Message::FindIntersect(points))) => (
                outcome().result(ResponderResult::FindIntersect(points)),
                Intersect,
            ),
            (Intersect, Input::Local(ResponderAction::IntersectFound(point, tip))) => (
                outcome().send(Message::IntersectFound(point, tip)),
                Idle {
                    send_rollback: true,
                },
            ),
            (Intersect, Input::Local(ResponderAction::IntersectNotFound(tip))) => (
                outcome().send(Message::IntersectNotFound(tip)),
                Idle {
                    send_rollback: false,
                },
            ),
            (Idle { send_rollback }, Input::Remote(Message::RequestNext)) => (
                outcome().result(ResponderResult::RequestNext),
                CanAwait { send_rollback },
            ),
            (CanAwait { send_rollback }, Input::Local(ResponderAction::AwaitReply)) => (
                outcome().send(Message::AwaitReply),
                MustReply { send_rollback },
            ),
            (
                CanAwait { .. } | MustReply { .. },
                Input::Local(ResponderAction::RollForward(content, tip)),
            ) => (
                outcome().send(Message::RollForward(content, tip)),
                Idle {
                    send_rollback: false,
                },
            ),
            (
                CanAwait { .. } | MustReply { .. },
                Input::Local(ResponderAction::RollBackward(point, tip)),
            ) => (
                outcome().send(Message::RollBackward(point, tip)),
                Idle {
                    send_rollback: false,
                },
            ),
            (this, input) => anyhow::bail!("invalid state: {:?} <- {:?}", this, input),
        })
    }
}
