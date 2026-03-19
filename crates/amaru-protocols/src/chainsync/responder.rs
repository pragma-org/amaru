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

use std::collections::{BTreeSet, VecDeque};

use amaru_kernel::{BlockHeader, EraName, HeaderHash, IsHeader, Peer, Point, Tip};
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
struct ChainFragment {
    rollback_to: Option<Point>,
    forwards: VecDeque<Point>,
}

impl ChainFragment {
    fn empty() -> Self {
        Self { rollback_to: None, forwards: VecDeque::new() }
    }

    fn set_rollback_to(&mut self, rollback_to: Point) {
        self.rollback_to = Some(rollback_to);
    }

    fn set_forward_points(&mut self, forwards: impl IntoIterator<Item = Point>) {
        self.forwards = forwards.into_iter().collect();
    }

    fn extend_forwards(&mut self, points: Vec<Point>) {
        let mut points: VecDeque<Point> = points.into();
        self.forwards.append(&mut points)
    }

    fn take_rollback(&mut self) -> Option<Point> {
        self.rollback_to.take()
    }

    fn pop_forward(&mut self) -> Option<Point> {
        self.forwards.pop_front()
    }

    fn is_at(&self, pointer: Point) -> bool {
        self.forwards.back() == Some(&pointer) || self.rollback_to == Some(pointer)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ChainSyncResponder {
    upstream: Tip,
    peer: Peer,
    pointer: Point,
    fragment: ChainFragment,
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
            ResponderState::Idle { send_rollback: false },
            Self { upstream, peer, pointer: Point::Origin, fragment: ChainFragment::empty(), conn_id, muxer },
        )
    }

    pub fn peer(&self) -> &Peer {
        &self.peer
    }

    pub fn pointer(&self) -> &Point {
        &self.pointer
    }

    pub fn upstream(&self) -> &Tip {
        &self.upstream
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
                let store = Store::new(eff.clone());
                let action = next_action(&mut self, &store, *proto)?;
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
                    self.fragment = ChainFragment::empty();
                }
                Ok((Some(action), self))
            }
            ResponderResult::RequestNext => {
                let action =
                    next_action(&mut self, &Store::new(eff.clone()), *proto).context("failed to get next action")?;
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

fn next_action(
    responder: &mut ChainSyncResponder,
    store: &dyn ReadOnlyChainStore<BlockHeader>,
    state: ResponderState,
) -> anyhow::Result<Option<ResponderAction>> {
    tracing::trace!(peer = %responder.peer(), pointer = %responder.pointer(), tip = %responder.upstream.point(), "computing the next header to return");
    match state {
        ResponderState::CanAwait { send_rollback: true } => {
            tracing::trace!(point = %responder.pointer, tip = %responder.upstream.point(), "emit rollback header after intersect");
            return Ok(Some(ResponderAction::RollBackward(responder.pointer, responder.upstream)));
        }
        ResponderState::MustReply | ResponderState::CanAwait { .. } => {}
        ResponderState::Idle { .. } | ResponderState::Intersect | ResponderState::Done => {
            return Ok(None);
        }
    };

    // The peer has now caught up
    if responder.pointer == responder.upstream.point() {
        tracing::trace!(peer = %responder.peer(), pointer = %responder.pointer(), tip = %responder.upstream.point(), "peer caught-up");
        return Ok((matches!(state, ResponderState::CanAwait { .. })).then_some(ResponderAction::AwaitReply));
    }

    // The peer has not yet caught up.
    // If its pointer is before the current anchor then we can serve headers directly from the immutable best chain
    if let Some(action) = next_action_from_immutable_best_chain(responder, store)? {
        return Ok(Some(action));
    }

    // The peer has not yet caught up, and its pointer is after the anchor.
    // We compute a chain fragment that is a snapshot of the best chain from 'pointer' to 'upstream'
    // if not already computed and return headers from it.
    // We need to keep track of that chain fragment because in between 2 initiator requests we could
    // have switched to another best chain and we need to remember which chain we were following to
    // provide a rollback point.
    next_action_from_chain_fragment(responder, store)
}

/// Return the next action from the immutable best chain
/// if the peer pointer is before the best chain anchor. Following points with `next_best_chain`
/// is safe there since those points can not change.
fn next_action_from_immutable_best_chain(
    responder: &mut ChainSyncResponder,
    store: &dyn ReadOnlyChainStore<BlockHeader>,
) -> anyhow::Result<Option<ResponderAction>> {
    let anchor_hash = store.get_anchor_hash();
    let anchor_point = get_header(store, &anchor_hash).point();
    if responder.pointer.slot_or_default() < anchor_point.slot_or_default() {
        #[expect(clippy::expect_used)]
        let point = store
            .next_best_chain(&responder.pointer)
            .expect("the pointer is on the best chain, before the anchor, there must be a next point on that chain");

        let header = get_header(store, &point.hash());
        tracing::trace!(
            peer = %responder.peer(),
            pointer = %point,
            anchor = %anchor_point,
            tip = %responder.upstream.point(),
            "emit forward header while streaming towards anchor"
        );
        responder.pointer = point;
        Ok(Some(ResponderAction::RollForward(HeaderContent::new(&header, EraName::Conway), responder.upstream)))
    } else {
        Ok(None)
    }
}

/// Return the next action from a computed ChainFragment.
/// Recompute or extend the current chain fragment if necessary (the tip might have moved).
fn next_action_from_chain_fragment(
    responder: &mut ChainSyncResponder,
    store: &dyn ReadOnlyChainStore<BlockHeader>,
) -> anyhow::Result<Option<ResponderAction>> {
    if responder.pointer == responder.upstream.point() {
        return Ok(None);
    }

    update_chain_fragment(responder, store)?;

    // Return a rollback point if the peer must first rollback
    if let Some(point) = responder.fragment.take_rollback() {
        responder.pointer = point;
        tracing::trace!(
            peer = %responder.peer(),
            pointer = %point,
            tip = %responder.upstream.point(),
            "emit rollback header"
        );
        return Ok(Some(ResponderAction::RollBackward(point, responder.upstream)));
    }

    // Otherwise return a forward header
    if let Some(point) = responder.fragment.pop_forward() {
        let header = get_header(store, &point.hash());
        responder.pointer = point;
        tracing::trace!(
            peer = %responder.peer(),
            pointer = %point,
            tip = %responder.upstream.point(),
            "emit forward header"
        );
        Ok(Some(ResponderAction::RollForward(HeaderContent::new(&header, EraName::Conway), responder.upstream)))
    } else {
        Err(anyhow::anyhow!("there should be at least one action to return"))
    }
}

/// Update the chain fragment that describes the list of headers to return for going from 'pointer'
/// to 'upstream'. This might just extend the existing fragment with new 'forwards' points if there are
/// new tips on the same best chain.
fn update_chain_fragment(
    responder: &mut ChainSyncResponder,
    store: &dyn ReadOnlyChainStore<BlockHeader>,
) -> Result<(), amaru_ouroboros::StoreError> {
    // Early exit if there's nothing to do
    if responder.pointer == responder.upstream.point() {
        responder.fragment = ChainFragment::empty();
        return Ok(());
    }

    let mut from_visited: BTreeSet<Point> = BTreeSet::new();
    let mut to_visited: BTreeSet<Point> = BTreeSet::new();

    from_visited.insert(responder.pointer);
    to_visited.insert(responder.upstream.point());

    let mut from_current = responder.pointer;
    let mut to_current = responder.upstream.point();
    let mut to_ancestors = vec![to_current];

    // We visit ancestors of from and to, until there's an intersection point
    let intersection_point = loop {
        let parent = get_parent_point(store, to_current)?;
        to_current = parent;

        // If the new responder.upstream tip extends the current fragment target, extend it with the new
        // points that can reach it.
        if responder.fragment.is_at(to_current) {
            // ancestors have been accumulated from tip to target,
            // we need to reverse them to extend the fragment forwards from the target
            to_ancestors.reverse();
            responder.fragment.extend_forwards(to_ancestors);
            return Ok(());
        }

        // Otherwise we continue iterating on ancestors for 'to' and 'from'
        to_ancestors.push(to_current);
        to_visited.insert(to_current);

        if from_visited.contains(&to_current) {
            break to_current;
        }

        let parent = get_parent_point(store, from_current)?;
        from_current = parent;
        from_visited.insert(from_current);

        if to_visited.contains(&from_current) {
            break from_current;
        }
    };

    if let Some(rollback_to) = (intersection_point != responder.pointer).then_some(intersection_point) {
        responder.fragment.set_rollback_to(rollback_to);
    }
    let intersection_index = to_ancestors.iter().position(|p| *p == intersection_point).unwrap_or(to_ancestors.len());
    let forwards: VecDeque<Point> = to_ancestors[..intersection_index].iter().rev().copied().collect();

    responder.fragment.set_forward_points(forwards);
    Ok(())
}

fn intersect(
    points: Vec<Point>,
    store: &dyn ReadOnlyChainStore<BlockHeader>,
    tip: Tip,
) -> anyhow::Result<ResponderAction> {
    Ok(match best_chain_intersection(store, &points)? {
        Some(point) => ResponderAction::IntersectFound(point, tip),
        None => ResponderAction::IntersectNotFound(tip),
    })
}

/// Find the most recent point that intersects the node best chain.
/// Since the best chain might be changing while we perform this search, we split the search by splitting
/// the input points into two categories:
///
///  - Points that are above the current anchor.
///  - And points that are at or before the current anchor.
///
/// 1. For the points above the anchor, we take a snapshot of the best chain tip and try to
///    find if one of the points is part of the tip ancestors => this is an intersection point.
///    Since iterating over the tip ancestors could be costly, we first try to see if one of the
///    points could be part of the current best chain by using the `load_from_best_chain` function.
///    If the best chain didn't change too much, this is a faster way to find an intersection point.
///
/// 2. For the points below the anchor, we can directly use `load_from_best_chain` to check if one
///    of the points is part of the best chain since those points are immutable.
///
fn best_chain_intersection(
    store: &dyn ReadOnlyChainStore<BlockHeader>,
    points: &[Point],
) -> Result<Option<Point>, amaru_ouroboros::StoreError> {
    if points.is_empty() {
        return Ok(None);
    }

    let anchor_hash = store.get_anchor_hash();
    let anchor_point = get_header(store, &anchor_hash).point();
    let best_tip = store.get_best_chain_hash();
    let mut requested_origin = false;

    let mut candidates: Vec<Point> = points.to_vec();
    candidates.sort_by_key(|point| std::cmp::Reverse(*point));
    candidates.dedup();

    let mut above_anchor = Vec::new();
    let mut at_or_before_anchor = Vec::new();
    for point in candidates {
        if point == Point::Origin {
            requested_origin = true;
        } else if point.slot_or_default() > anchor_point.slot_or_default() {
            above_anchor.push(point);
        } else {
            at_or_before_anchor.push(point);
        }
    }

    for point in &above_anchor {
        if store.load_from_best_chain(point).is_some() {
            return Ok(Some(*point));
        }
    }

    let above_anchor: BTreeSet<Point> = above_anchor.into_iter().collect();

    for hash in store.ancestors_hashes(&best_tip) {
        let point = get_header(store, &hash).point();
        if point.slot_or_default() <= anchor_point.slot_or_default() {
            break;
        }
        if above_anchor.contains(&point) {
            return Ok(Some(point));
        }
    }

    for point in at_or_before_anchor {
        if store.load_from_best_chain(&point).is_some() {
            return Ok(Some(point));
        }
    }

    if requested_origin {
        return Ok(Some(Point::Origin));
    }

    Ok(None)
}

/// Return the header for a given header hash when the header is expected to exist in the store.
#[expect(clippy::expect_used)]
fn get_header(store: &dyn ReadOnlyChainStore<BlockHeader>, hash: &HeaderHash) -> BlockHeader {
    store.load_header(hash).expect("cannot load header")
}

/// Return the point correponding to a parent header
fn get_parent_point(
    store: &dyn ReadOnlyChainStore<BlockHeader>,
    point: Point,
) -> Result<Point, amaru_ouroboros::StoreError> {
    if point == Point::Origin {
        return Ok(Point::Origin);
    }

    let header = get_header(store, &point.hash());
    match header.parent() {
        Some(parent_hash) => Ok(store.load_header(&parent_hash).map(|parent| parent.point()).unwrap_or(Point::Origin)),
        None => Ok(Point::Origin),
    }
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

    use amaru_kernel::{BlockHeader, Hash, IsHeader, Slot, from_cbor, make_header, size::HEADER};
    use amaru_ouroboros_traits::{
        ChainStore, in_memory_consensus_store::InMemConsensusStore, overriding_consensus_store::OverridingChainStore,
    };

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

    #[test]
    fn best_chain_intersection_returns_origin_when_requested() {
        let (store, _points) = build_chain_store(10, 0);

        let result = best_chain_intersection(store.as_ref(), &[Point::Origin]).unwrap();

        assert_eq!(result, Some(Point::Origin));
    }

    #[test]
    fn best_chain_intersection_prefers_newer_match_over_origin() {
        let (store, points) = build_chain_store(10, 0);

        let result = best_chain_intersection(store.as_ref(), &[Point::Origin, points[7]]).unwrap();

        assert_eq!(result, Some(points[7]));
    }

    #[test]
    fn best_chain_intersection_falls_back_to_snapshotted_tip_scan_above_anchor() {
        let (store, points) = build_chain_store(10, 5);
        let anchor_slot = points[5].slot_or_default();
        let store: Arc<dyn ChainStore<BlockHeader>> = store;
        let overridden = OverridingChainStore::builder(store)
            .with_load_from_best_chain(move |inner, point| {
                if point.slot_or_default() > anchor_slot { None } else { inner.load_from_best_chain(point) }
            })
            .build();

        let result = best_chain_intersection(&overridden, &[points[7]]).unwrap();

        assert_eq!(result, Some(points[7]));
    }

    #[test]
    fn best_chain_intersection_falls_back_to_pre_anchor_lookup_when_tip_scan_misses() {
        let (store, points) = build_chain_store(10, 5);
        let anchor_slot = points[5].slot_or_default();
        let store: Arc<dyn ChainStore<BlockHeader>> = store;
        let overridden = OverridingChainStore::builder(store)
            .with_load_from_best_chain(move |inner, point| {
                if point.slot_or_default() > anchor_slot { None } else { inner.load_from_best_chain(point) }
            })
            .build();

        let unknown = Point::Specific(Slot::from(999), Hash::new([0xff; HEADER]));
        let result = best_chain_intersection(&overridden, &[unknown, points[2]]).unwrap();

        assert_eq!(result, Some(points[2]));
    }

    #[test]
    fn update_chain_fragment_when_peer_is_at_tip_clears_the_fragment() {
        let (store, points) = build_chain_store(4, 0);
        let mut responder = make_responder(Tip::new(points[1], 0.into()));
        responder.pointer = points[1];

        update_chain_fragment(&mut responder, store.as_ref()).unwrap();

        assert_eq!(responder.fragment, ChainFragment::empty());
    }

    #[test]
    fn update_chain_fragment_from_anchor() {
        let (store, points) = build_chain_store(3, 0);
        let mut responder = make_responder(make_tip(&points));

        update_chain_fragment(&mut responder, store.as_ref()).unwrap();
        let mut expected = ChainFragment::empty();
        expected.set_forward_points(vec![points[0], points[1], points[2]]);
        assert_eq!(responder.fragment, expected);
    }

    #[test]
    fn update_chain_fragment_when_forward_points_are_extended() {
        let (store, points) = build_chain_store(4, 0);
        let mut responder = make_responder(make_tip(&points));
        // The peer starts at 1, it needs 2 and 3 to get to the tip
        responder.pointer = points[1];

        update_chain_fragment(&mut responder, store.as_ref()).unwrap();

        let mut expected = ChainFragment::empty();
        expected.set_forward_points(vec![points[2], points[3]]);
        assert_eq!(responder.fragment, expected);
    }

    #[test]
    fn update_chain_fragment_rollback_on_same_chain() {
        let (store, points) = build_chain_store(4, 0);
        let mut responder = make_responder(Tip::new(points[1], 0.into()));
        responder.pointer = points[3];

        update_chain_fragment(&mut responder, store.as_ref()).unwrap();

        let mut expected = ChainFragment::empty();
        expected.set_rollback_to(points[1]);
        assert_eq!(responder.fragment, expected);
    }

    #[test]
    fn next_action_streams_incrementally_from_origin_to_anchor() {
        // We stream the first point of the chain if the intersection is at origin
        // 0 ---- 5 ----- 10
        // ^      ^        ^
        // peer  anchor   tip

        let (store, points) = build_chain_store(10, 5);
        let tip = make_tip(&points);
        let mut responder = make_responder(tip);

        let action = next_action(&mut responder, store.as_ref(), ResponderState::MustReply).unwrap().unwrap();

        assert_eq!(responder.pointer, points[0]);
        assert_eq!(responder.fragment, ChainFragment::empty());
        assert_is_rollforward(&action, points[0], tip);
    }

    #[test]
    fn next_action_streams_incrementally_before_anchor() {
        // Before
        // 0 ---- 2 --- 5 ----- 10
        //        ^     ^        ^
        //      peer  anchor   tip
        let (store, points) = build_chain_store(10, 5);
        let tip = make_tip(&points);
        let mut responder = make_responder(tip);
        responder.pointer = points[2];

        let action = next_action(&mut responder, store.as_ref(), ResponderState::MustReply).unwrap().unwrap();

        // After
        // 0 ---- 2 - 3 --- 5 ----- 10
        //            ^     ^        ^
        //           peer  anchor   tip
        assert_eq!(responder.pointer, points[3]);
        // no need to store a chain fragment because we are streaming the immutable part of the best chain
        assert_eq!(responder.fragment, ChainFragment::empty());
        assert_is_rollforward(&action, points[3], tip);
    }

    #[test]
    fn next_action_switches_to_fragment_at_anchor() {
        // Before
        // 0 ---- 2 --- 5 ----- 10
        //              ^       ^
        //            anchor    tip
        //            peer
        let (store, points) = build_chain_store(10, 5);
        let tip = make_tip(&points);
        let mut responder = make_responder(tip);
        responder.pointer = points[5];

        let action = next_action(&mut responder, store.as_ref(), ResponderState::MustReply).unwrap().unwrap();

        // After
        // 0 ---- 2 --- 5 -- 6 ---- 10
        //              ^    ^       ^
        //            anchor |      tip
        //                  peer
        assert_eq!(responder.pointer, points[6]);

        let mut expected = ChainFragment::empty();
        // We will send all the headers to the tip even if it has moved in the meantime in the database
        expected.set_forward_points(vec![points[7], points[8], points[9]]);
        assert_eq!(responder.fragment, expected);
        assert_is_rollforward(&action, points[6], tip);
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

        for slot in 1..=n {
            let header_raw = make_header(slot, slot, prev_hash);
            let hash = Hash::new([slot as u8; HEADER]);
            let header = BlockHeader::new(header_raw, hash);
            store.store_header(&header).unwrap();
            let point = header.point();
            store.roll_forward_chain(&point).unwrap();
            points.push(point);
            prev_hash = Some(header.hash());
        }

        store.set_anchor_hash(&points[anchor_index as usize].hash()).unwrap();
        store.set_best_chain_hash(&points.last().unwrap().hash()).unwrap();
        (store, points)
    }

    fn make_tip(points: &[Point]) -> Tip {
        let last = points.last().unwrap();
        Tip::new(*last, 0.into())
    }

    fn make_responder(upstream: Tip) -> ChainSyncResponder {
        ChainSyncResponder {
            upstream,
            peer: Peer::new("peer"),
            pointer: Point::Origin,
            fragment: ChainFragment::empty(),
            conn_id: ConnectionId::initial(),
            muxer: StageRef::named_for_tests("muxer"),
        }
    }

    fn assert_is_rollforward(action: &ResponderAction, expected: Point, expected_tip: Tip) {
        if let ResponderAction::RollForward(content, tip) = action {
            assert_eq!(*tip, expected_tip);
            let header: BlockHeader = from_cbor(&content.cbor).expect("should decode roll-forward header");
            assert_eq!(header.point().slot_or_default(), expected.slot_or_default());
        } else {
            panic!("expected RollForward, got {action:?}");
        }
    }
}
