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

use std::{cmp::Reverse, sync::Arc};

use amaru_kernel::{BlockHeader, EraHistory, IsHeader, ORIGIN_HASH, Peer, Point, Tip};
use amaru_ouroboros::{ConnectionId, ReadOnlyChainStore};
use anyhow::{Context, ensure};
use pure_stage::{DeserializerGuards, Effects, StageRef, Void};
use tracing::instrument;

use crate::{
    chainsync::messages::{HeaderContent, Message},
    mux::MuxMessage,
    protocol::{
        Inputs, Miniprotocol, Outcome, PROTO_N2N_CHAIN_SYNC, ProtocolState, Responder, StageState, miniprotocol,
        outcome,
    },
    store_effects::Store,
};

pub fn register_deserializers() -> DeserializerGuards {
    vec![
        pure_stage::register_data_deserializer::<ResponderMessage>().boxed(),
        pure_stage::register_data_deserializer::<(ResponderState, ChainSyncResponder)>().boxed(),
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

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ChainSyncResponder {
    upstream: Tip,
    peer: Peer,
    pointer: Point,
    conn_id: ConnectionId,
    muxer: StageRef<MuxMessage>,
    era_history: Arc<EraHistory>,
}

impl ChainSyncResponder {
    pub fn new(
        upstream: Tip,
        peer: Peer,
        conn_id: ConnectionId,
        muxer: StageRef<MuxMessage>,
        era_history: Arc<EraHistory>,
    ) -> (ResponderState, Self) {
        (ResponderState::Idle { send_rollback: false }, Self { upstream, peer, pointer: Point::Origin, conn_id, muxer, era_history })
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
                tracing::trace!(%tip, "New tip");
                self.upstream = tip;
                let action = next_header(*proto, &mut self.pointer, &Store::new(eff.clone()), self.upstream, &self.era_history)
                    .context("failed to get next header")?;
                Ok((action, self))
            }
        }
    }

    #[instrument(level = "debug", name = "chainsync.responder.stage", skip_all, fields(message_type = input.message_type()))]
    async fn network(
        mut self,
        proto: &ResponderState,
        input: ResponderResult,
        eff: &Effects<Inputs<Self::LocalIn>>,
    ) -> anyhow::Result<(Option<ResponderAction>, Self)> {
        match input {
            ResponderResult::FindIntersect(points) => {
                let action = intersect(points, &Store::new(eff.clone()), self.upstream)
                    .context("failed to find intersection")?;
                if let ResponderAction::IntersectFound(point, _tip) = &action {
                    self.pointer = *point;
                }
                Ok((Some(action), self))
            }
            ResponderResult::RequestNext => {
                let action = next_header(*proto, &mut self.pointer, &Store::new(eff.clone()), self.upstream, &self.era_history)
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
    pointer: &mut Point,
    store: &dyn ReadOnlyChainStore<BlockHeader>,
    tip: Tip,
    era_history: &EraHistory,
) -> anyhow::Result<Option<ResponderAction>> {
    match state {
        ResponderState::CanAwait { send_rollback: true } => {
            return Ok(Some(ResponderAction::RollBackward(*pointer, tip)));
        }
        ResponderState::MustReply | ResponderState::CanAwait { .. } => {}
        ResponderState::Idle { .. } | ResponderState::Intersect | ResponderState::Done => {
            return Ok(None);
        }
    };
    if *pointer == tip.point() {
        return Ok((matches!(state, ResponderState::CanAwait { .. })).then_some(ResponderAction::AwaitReply));
    }

    if store.load_from_best_chain(pointer).is_none() {
        // client is on a different fork, we need to roll backward
        return next_header_rollback(pointer, store, tip);
    }
    // pointer is on the best chain, we need to roll forward
    let Some(point) = store.next_best_chain(pointer) else {
        return Ok(None);
    };
    let header =
        store.load_header(&point.hash()).ok_or_else(|| anyhow::anyhow!("best-chain header not found: {}", point))?;
    // Validate that the header's parent matches our pointer; the store can change between
    // load_from_best_chain and next_best_chain, so the "next" block may no longer be correct.
    let expected_parent = pointer.hash();
    let actual_parent = header.parent_hash().unwrap_or(ORIGIN_HASH);
    if actual_parent != expected_parent {
        // Best chain changed; fall back to searching backwards from the advertised tip
        return next_header_from_tip(pointer, store, tip);
    }
    *pointer = point;
    let era_name = era_history.slot_to_era_tag(header.slot())?;
    Ok(Some(ResponderAction::RollForward(HeaderContent::new(&header, era_name), tip)))
}

/// Rollback when the client pointer is on a different fork from our best chain.
fn next_header_rollback(
    pointer: &mut Point,
    store: &dyn ReadOnlyChainStore<BlockHeader>,
    tip: Tip,
) -> anyhow::Result<Option<ResponderAction>> {
    let header = store.load_header(&pointer.hash()).ok_or_else(|| anyhow::anyhow!("remote pointer not found"))?;
    for header in store.ancestors(header) {
        if store.load_from_best_chain(&header.point()).is_some() {
            *pointer = header.point();
            return Ok(Some(ResponderAction::RollBackward(header.point(), tip)));
        }
    }
    anyhow::bail!("no overlap found between client pointer chain and stored best chain");
}

/// Find the next action by searching backwards from the advertised tip.
/// Used when next_best_chain would return a header whose parent doesn't match the pointer
/// (e.g. because the store changed between load_from_best_chain and next_best_chain).
fn next_header_from_tip(
    pointer: &mut Point,
    store: &dyn ReadOnlyChainStore<BlockHeader>,
    tip: Tip,
) -> anyhow::Result<Option<ResponderAction>> {
    let tip_header = store
        .load_header(&tip.point().hash())
        .ok_or_else(|| anyhow::anyhow!("tip header not found: {}", tip.point()))?;
    let mut tip_chain = store.ancestors(tip_header).map(|h| h.point()).peekable();
    let pointer_header =
        store.load_header(&pointer.hash()).ok_or_else(|| anyhow::anyhow!("remote pointer not found"))?;
    for point in store.ancestors(pointer_header).map(|h| h.point()) {
        while let Some(tip_point) = tip_chain.peek() {
            if tip_point.slot_or_default() > point.slot_or_default() {
                tip_chain.next();
            } else {
                break;
            }
        }
        if let Some(tip_point) = tip_chain.peek() {
            if *tip_point == point {
                *pointer = point;
                return Ok(Some(ResponderAction::RollBackward(point, tip)));
            }
        } else {
            break;
        }
    }
    Err(anyhow::anyhow!("no overlap found between client pointer chain and tip chain"))
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

    for point in &points {
        if store.load_from_best_chain(point).is_some() {
            return Ok(ResponderAction::IntersectFound(*point, tip));
        }
    }
    Ok(ResponderAction::IntersectNotFound(tip))
}

#[derive(Debug, PartialEq, Eq)]
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

impl ResponderResult {
    fn message_type(&self) -> &'static str {
        match self {
            ResponderResult::FindIntersect(_) => "FindIntersect",
            ResponderResult::RequestNext => "RequestNext",
            ResponderResult::Done => "Done",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Ord, PartialOrd)]
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
    type Error = Void;

    fn init(&self) -> anyhow::Result<(Outcome<Self::WireMsg, Self::Out, Self::Error>, Self)> {
        Ok((outcome().want_next(), *self))
    }

    #[instrument(level = "debug", name = "chainsync.responder.protocol", skip_all, fields(message_type = input.message_type()))]
    fn network(&self, input: Self::WireMsg) -> anyhow::Result<(Outcome<Self::WireMsg, Self::Out, Self::Error>, Self)> {
        use ResponderState::*;

        Ok(match (self, input) {
            (Idle { .. }, Message::FindIntersect(points)) => {
                (outcome().result(ResponderResult::FindIntersect(points)), Intersect)
            }
            (Idle { send_rollback }, Message::RequestNext(1)) => {
                (outcome().result(ResponderResult::RequestNext), CanAwait { send_rollback: *send_rollback })
            }
            (Idle { .. }, Message::Done) => (outcome().result(ResponderResult::Done), Done),
            (this, input) => anyhow::bail!("invalid state: {:?} <- {:?}", this, input),
        })
    }

    fn local(&self, input: Self::Action) -> anyhow::Result<(Outcome<Self::WireMsg, Void, Self::Error>, Self)> {
        use ResponderState::*;

        Ok(match (self, input) {
            (Intersect, ResponderAction::IntersectFound(point, tip)) => {
                (outcome().send(Message::IntersectFound(point, tip)).want_next(), Idle { send_rollback: true })
            }
            (Intersect, ResponderAction::IntersectNotFound(tip)) => {
                (outcome().send(Message::IntersectNotFound(tip)).want_next(), Idle { send_rollback: false })
            }
            (CanAwait { send_rollback }, ResponderAction::AwaitReply) => {
                ensure!(!*send_rollback, "cannot AwaitReply after intersect");
                (outcome().send(Message::AwaitReply), MustReply)
            }
            (CanAwait { send_rollback }, ResponderAction::RollForward(content, tip)) => {
                ensure!(!*send_rollback, "cannot RollForward after intersect");
                (outcome().send(Message::RollForward(content, tip)).want_next(), Idle { send_rollback: false })
            }
            (MustReply, ResponderAction::RollForward(content, tip)) => {
                (outcome().send(Message::RollForward(content, tip)).want_next(), Idle { send_rollback: false })
            }
            (CanAwait { .. } | MustReply, ResponderAction::RollBackward(point, tip)) => {
                (outcome().send(Message::RollBackward(point, tip)).want_next(), Idle { send_rollback: false })
            }
            (this, input) => anyhow::bail!("invalid state: {:?} <- {:?}", this, input),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use amaru_kernel::{BlockHeader, EraName, Hash, Slot, make_header, size::HEADER};
    use amaru_ouroboros_traits::{ChainStore, in_memory_consensus_store::InMemConsensusStore};

    use super::*;
    use crate::{chainsync::initiator::InitiatorState, protocol::ProtoSpec};

    #[test]
    fn intersect_finds_point_on_best_chain() {
        let (store, points) = build_chain_store(10, 0);
        let tip = make_tip(&points);

        let result = intersect(vec![points[5]], store.as_ref(), tip).unwrap();
        assert_eq!(result, ResponderAction::IntersectFound(points[5], tip));
    }

    #[test]
    fn intersect_returns_most_recent_matching_point() {
        let (store, points) = build_chain_store(10, 0);
        let tip = make_tip(&points);

        // points are sorted highest-first, so point[7] should be found first
        let result = intersect(vec![points[3], points[7]], store.as_ref(), tip).unwrap();
        assert_eq!(result, ResponderAction::IntersectFound(points[7], tip));
    }

    #[test]
    fn intersect_finds_point_before_anchor() {
        // Anchor at index 5, but point[2] is still on the best chain index
        let (store, points) = build_chain_store(10, 5);
        let tip = make_tip(&points);

        let result = intersect(vec![points[2]], store.as_ref(), tip).unwrap();
        assert_eq!(result, ResponderAction::IntersectFound(points[2], tip));
    }

    #[test]
    fn intersect_not_found_with_empty_points() {
        let (store, points) = build_chain_store(10, 0);
        let tip = make_tip(&points);

        let result = intersect(vec![], store.as_ref(), tip).unwrap();
        assert_eq!(result, ResponderAction::IntersectNotFound(tip));
    }

    #[test]
    fn intersect_not_found_with_unknown_points() {
        let (store, points) = build_chain_store(10, 0);
        let tip = make_tip(&points);

        let unknown = Point::Specific(Slot::from(999), Hash::new([0xff; HEADER]));
        let result = intersect(vec![unknown], store.as_ref(), tip).unwrap();
        assert_eq!(result, ResponderAction::IntersectNotFound(tip));
    }

    #[expect(clippy::wildcard_enum_match_arm)]
    #[test]
    fn test_responder_protocol() {
        use Message::{
            AwaitReply, FindIntersect, IntersectFound, IntersectNotFound, RequestNext, RollBackward, RollForward,
        };
        use ResponderState::{CanAwait, Done, Idle, Intersect, MustReply};

        // canonical states and messages
        let idle = |send_rollback: bool| Idle { send_rollback };
        let can_await = |send_rollback: bool| CanAwait { send_rollback };
        let find_intersect = || FindIntersect(vec![Point::Origin]);
        let intersect_found = || IntersectFound(Point::Origin, Tip::origin());
        let intersect_not_found = || IntersectNotFound(Tip::origin());
        let roll_forward = || RollForward(HeaderContent::with_bytes(vec![], EraName::Conway), Tip::origin());
        let roll_backward = || RollBackward(Point::Origin, Tip::origin());

        let mut spec = ProtoSpec::default();
        spec.init(idle(false), find_intersect(), Intersect);
        spec.init(idle(true), find_intersect(), Intersect);
        spec.init(idle(false), RequestNext(1), can_await(false));
        spec.init(idle(true), RequestNext(1), can_await(true));
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

        spec.check(idle(false), |msg| match msg {
            AwaitReply => Some(ResponderAction::AwaitReply),
            RollForward(header_content, tip) => Some(ResponderAction::RollForward(header_content.clone(), *tip)),
            RollBackward(point, tip) => Some(ResponderAction::RollBackward(*point, *tip)),
            IntersectFound(point, tip) => Some(ResponderAction::IntersectFound(*point, *tip)),
            IntersectNotFound(tip) => Some(ResponderAction::IntersectNotFound(*tip)),
            _ => None,
        });

        spec.assert_refines(&super::super::initiator::tests::spec(), |state| match state {
            Idle { .. } => InitiatorState::Idle,
            CanAwait { .. } => InitiatorState::CanAwait(0),
            MustReply => InitiatorState::MustReply(0),
            Intersect => InitiatorState::Intersect,
            Done => InitiatorState::Done,
        });
    }

    // HELPERS

    /// Build an in-memory chain store with `n` headers on the best chain,
    /// and set the anchor at `anchor_index`.
    fn build_chain_store(n: u64, anchor_index: u64) -> (Arc<InMemConsensusStore<BlockHeader>>, Vec<Point>) {
        let store = Arc::new(InMemConsensusStore::new());
        let mut points = Vec::new();
        let mut prev_hash = None;

        for slot in 0..n {
            let header_raw = make_header(slot, slot, prev_hash);
            let hash = Hash::new([slot as u8; HEADER]);
            let header = BlockHeader::new(header_raw, hash);
            store.store_header(&header).unwrap();
            let point = Point::Specific(Slot::from(slot), hash);
            store.roll_forward_chain(&point).unwrap();
            points.push(point);
            prev_hash = Some(hash);
        }

        store.set_anchor_hash(&points[anchor_index as usize].hash()).unwrap();
        store.set_best_chain_hash(&points.last().unwrap().hash()).unwrap();
        (store, points)
    }

    fn make_tip(points: &[Point]) -> Tip {
        let last = points.last().unwrap();
        Tip::new(*last, 0.into())
    }
}
