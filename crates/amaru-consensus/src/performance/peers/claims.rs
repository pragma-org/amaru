// Copyright 2026 PRAGMA
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

//! Header-claim index: who can serve a hash, and the parent links that walk uses.

use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};

use amaru_kernel::{BlockHeight, HeaderHash, Peer, Point};
use amaru_pure_stage::Instant;

use super::{
    PeerPerformance,
    quality::{PeerScores, rank_score},
    reputation::PeerShareFlags,
};

/// Safety cap on parent walks. With height-aware early exit this is rarely approached;
/// it only bounds pathological maps that lack usable height information.
const MAX_PARENT_WALK: usize = 512;

/// Why we believe a peer can serve a block at a given hash.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub enum ClaimKind {
    /// Chainsync `IntersectFound(current, _)`: peer shares `current`.
    Intersection,
    /// Validated chainsync header (first or duplicate announcement).
    HeaderAnnouncement,
    /// Peer successfully sent us this block body.
    BlockDelivery,
}

impl ClaimKind {
    fn strength(self) -> u8 {
        match self {
            ClaimKind::Intersection => 1,
            ClaimKind::HeaderAnnouncement => 2,
            ClaimKind::BlockDelivery => 3,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BlockClaim {
    pub hash: HeaderHash,
    pub height: BlockHeight,
    pub parent: Option<HeaderHash>,
    pub kind: ClaimKind,
    pub at: Instant,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PeerSnapshot {
    pub peer: Peer,
    pub scores: PeerScores,
    pub tips: Vec<BlockClaim>,
    pub share: PeerShareFlags,
}

/// Result of peer selection for a fetch batch.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FetchPeerSet {
    pub peers: Vec<Peer>,
    /// True when no peer had a covering claim (caller may fall back or wait).
    pub weak: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SelectPeersParams {
    /// Oldest-first chain fragment to fetch (parent before child).
    pub need: Vec<HeaderHash>,
    pub max_peers: usize,
    /// Peers already asked for this batch. They are skipped so a later wakeup returns the next
    /// covering peers rather than the same set.
    pub exclude: Vec<Peer>,
    /// Wall-clock for selection (reserved for future staleness-aware ranking).
    pub now: Instant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(super) struct ParentInfo {
    parent: Option<HeaderHash>,
    height: BlockHeight,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(super) struct ClaimMeta {
    height: BlockHeight,
    parent: Option<HeaderHash>,
    kind: ClaimKind,
    at: Instant,
}

/// Whether `peer` has a tip claim that implies they can serve block `target`.
///
/// True if some tip equals `target`, or walking parents from a tip reaches `target`
/// (claim on a descendant ⇒ ancestors available). Walks stop early once block height
/// shows the target has been missed (current height ≤ target height without a hash match).
///
/// `target` must appear in `parents` (every claim inserts itself and stubs its parent with
/// height). If it does not, no recorded claim can cover it, so this returns false.
fn peer_covers_hash(inner: &PeerPerformance, peer: &Peer, target: HeaderHash) -> bool {
    let Some(state) = inner.peers.get(peer) else {
        return false;
    };
    if state.tips.is_empty() {
        return false;
    }

    let Some(target_height) = inner.parents.get(&target).map(|info| info.height) else {
        return false;
    };

    for (claim_hash, meta) in &state.tips {
        let start = ParentInfo { parent: meta.parent, height: meta.height };
        if walk_reaches(&inner.parents, *claim_hash, start, target, target_height) {
            return true;
        }
    }
    false
}

/// Walk parent links from `start_hash` looking for `target` at known `target_height`.
///
/// `start` is the height/parent of the first node (a tip claim). Further steps use `parents`.
/// Stops once the walk is at or below `target_height` without matching `target` (the block
/// cannot lie further toward genesis on this branch).
fn walk_reaches(
    parents: &BTreeMap<HeaderHash, ParentInfo>,
    start_hash: HeaderHash,
    start: ParentInfo,
    target: HeaderHash,
    target_height: BlockHeight,
) -> bool {
    let mut walk = start_hash;
    let mut info = start;
    for _ in 0..MAX_PARENT_WALK {
        if walk == target {
            return true;
        }
        // Heights decrease toward genesis. At or below the target height with a
        // different hash means this branch has missed `target`.
        if info.height <= target_height {
            return false;
        }
        let Some(parent) = info.parent else {
            return false;
        };
        let Some(parent_info) = parents.get(&parent).copied() else {
            return false;
        };
        walk = parent;
        info = parent_info;
    }
    false
}

fn dominate_tips(
    parents: &BTreeMap<HeaderHash, ParentInfo>,
    tips: &mut BTreeMap<HeaderHash, ClaimMeta>,
    hash: HeaderHash,
    meta: ClaimMeta,
) {
    if tips.iter().any(|(tip_hash, _)| is_ancestor_of(parents, hash, *tip_hash) && hash != *tip_hash) {
        return;
    }

    let to_remove: Vec<HeaderHash> = tips.keys().copied().filter(|tip| is_ancestor_of(parents, *tip, hash)).collect();
    for tip in to_remove {
        tips.remove(&tip);
    }
    tips.insert(hash, meta);
}

fn is_ancestor_of(
    parents: &BTreeMap<HeaderHash, ParentInfo>,
    maybe_ancestor: HeaderHash,
    descendant: HeaderHash,
) -> bool {
    if maybe_ancestor == descendant {
        return true;
    }
    // Both ends of domination checks are claim hashes already present in `parents`.
    let Some(ancestor_height) = parents.get(&maybe_ancestor).map(|info| info.height) else {
        return false;
    };
    let mut walk = descendant;
    for _ in 0..MAX_PARENT_WALK {
        if walk == maybe_ancestor {
            return true;
        }
        let Some(info) = parents.get(&walk) else {
            return false;
        };
        if info.height <= ancestor_height {
            return false;
        }
        match info.parent {
            Some(parent) => walk = parent,
            None => return false,
        }
    }
    false
}

impl PeerPerformance {
    pub fn record_intersection(&mut self, peer: Peer, current: Point, parent: Option<HeaderHash>, at: Instant) {
        self.insert_claim(peer, current.hash(), current.block_height(), parent, ClaimKind::Intersection, at);
    }

    pub fn record_header_announcement(&mut self, peer: Peer, header: Point, parent: Option<HeaderHash>, at: Instant) {
        // First announcer records zero lag (bonus); later peers record delay vs first.
        let lag = self
            .first_announced_at(&header.hash())
            .map(|(_, first_at)| at.saturating_since(first_at))
            .unwrap_or(Duration::ZERO);
        self.update_header_lag(&peer, lag, at);
        self.insert_claim(peer, header.hash(), header.block_height(), parent, ClaimKind::HeaderAnnouncement, at);
    }

    #[expect(clippy::too_many_arguments)]
    pub fn record_block_delivery(
        &mut self,
        peer: Peer,
        hash: HeaderHash,
        height: BlockHeight,
        parent: Option<HeaderHash>,
        at: Instant,
        response: Duration,
        bytes: u64,
    ) {
        self.insert_claim(peer, hash, height, parent, ClaimKind::BlockDelivery, at);
        self.update_block_delivery(&peer, response, bytes, at);
    }

    pub fn clear_availability(&mut self, peer: &Peer) {
        self.clear_peer_claims(peer);
    }

    fn insert_claim(
        &mut self,
        peer: Peer,
        hash: HeaderHash,
        height: BlockHeight,
        parent: Option<HeaderHash>,
        kind: ClaimKind,
        at: Instant,
    ) {
        if let Some(existing) = self.parents.get(&hash)
            && existing.height != height
        {
            return;
        }
        // Always refresh this node's parent link (stubs from children may have `parent: None`).
        self.parents.insert(hash, ParentInfo { parent, height });
        // Record the parent with height so coverage walks always have a target height.
        // On a valid chain the parent block height is exactly one less than the child.
        if let Some(p) = parent {
            self.parents.entry(p).or_insert(ParentInfo { parent: None, height: height - 1 });
        }

        let meta = ClaimMeta { height, parent, kind, at };
        let claimants = self.direct.entry(hash).or_default();
        match claimants.get_mut(&peer) {
            Some(existing) => {
                if kind.strength() > existing.kind.strength() {
                    existing.kind = kind;
                }
                if at < existing.at {
                    existing.at = at;
                }
                existing.height = height;
                existing.parent = parent;
            }
            None => {
                claimants.insert(peer, meta.clone());
            }
        }

        self.claim_index.entry(peer).or_default().insert(hash);
        self.note_activity(peer, at);
        let state = self.peers.entry(peer).or_default();
        self::dominate_tips(&self.parents, &mut state.tips, hash, meta);
    }

    pub fn first_announced_at(&self, hash: &HeaderHash) -> Option<(Peer, Instant)> {
        let claimants = self.direct.get(hash)?;
        claimants.iter().map(|(p, m)| (*p, m.at)).min_by_key(|(_, at)| *at)
    }

    pub fn direct_claimants(&self, hash: &HeaderHash) -> Vec<(Peer, Instant, ClaimKind)> {
        match self.direct.get(hash) {
            Some(claimants) => claimants.iter().map(|(p, m)| (*p, m.at, m.kind)).collect(),
            None => Vec::new(),
        }
    }

    /// Whether the peer can serve every block in `need`.
    ///
    /// Equivalently: can serve the **last** hash of the fragment. A claim on (or above) that
    /// point implies all ancestors, so partial-prefix peers are not treated as range-capable —
    /// selecting them would leave blockfetch short of the requested range until timeout.
    pub fn peer_covers_fragment(&self, peer: &Peer, need: &[HeaderHash]) -> bool {
        let Some(last) = need.last() else {
            return true;
        };
        peer_covers_hash(self, peer, *last)
    }

    pub fn select_peers_for_fetch(&self, params: SelectPeersParams) -> FetchPeerSet {
        let SelectPeersParams { need, max_peers, exclude, now: _ } = params;
        if need.is_empty() || max_peers == 0 {
            return FetchPeerSet { peers: Vec::new(), weak: true };
        }
        let excluded: BTreeSet<Peer> = exclude.into_iter().collect();

        let mut ranked: Vec<(f64, Peer)> = Vec::new();
        for &peer in self.peers.keys() {
            if excluded.contains(&peer) || !self.peer_covers_fragment(&peer, &need) {
                continue;
            }
            let score = rank_score(self.peers.get(&peer).map(|s| &s.scores), need.len());
            ranked.push((score, peer));
        }

        ranked.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.1.cmp(&b.1)));

        if ranked.is_empty() {
            return FetchPeerSet { peers: Vec::new(), weak: true };
        }

        let peers: Vec<Peer> = ranked.into_iter().take(max_peers).map(|(_, p)| p).collect();
        FetchPeerSet { peers, weak: false }
    }

    pub fn snapshot(&self, peer: &Peer) -> Option<PeerSnapshot> {
        let state = self.peers.get(peer)?;
        let tips = state
            .tips
            .iter()
            .map(|(hash, meta)| BlockClaim {
                hash: *hash,
                height: meta.height,
                parent: meta.parent,
                kind: meta.kind,
                at: meta.at,
            })
            .collect();
        Some(PeerSnapshot { peer: *peer, scores: state.scores.clone(), tips, share: state.share_flags() })
    }

    pub fn record_rollback(&mut self, peer: Peer, point: Point, parent: Option<HeaderHash>, at: Instant) {
        let hash = point.hash();
        let height = point.block_height();
        self.parents.entry(hash).or_insert(ParentInfo { parent, height });

        if let Some(state) = self.peers.get_mut(&peer) {
            state.tips.clear();
        }
        self.insert_claim(peer, hash, height, parent, ClaimKind::HeaderAnnouncement, at);
    }

    pub fn prune_below(&mut self, min_height: BlockHeight) {
        self.parents.retain(|_, info| info.height >= min_height);
        let parents = &self.parents;
        self.direct.retain(|hash, claimants| parents.contains_key(hash) && !claimants.is_empty());
        for state in self.peers.values_mut() {
            state.tips.retain(|hash, meta| meta.height >= min_height && parents.contains_key(hash));
        }
        self.claim_index.retain(|peer, hashes| {
            hashes.retain(|hash| self.direct.get(hash).is_some_and(|claimants| claimants.contains_key(peer)));
            !hashes.is_empty()
        });
    }
}
