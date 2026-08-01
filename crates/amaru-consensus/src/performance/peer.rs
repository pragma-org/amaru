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

//! Peer availability claims and quality scores for fetch subset selection and churn.

use std::{collections::BTreeMap, time::Duration};

use amaru_kernel::{BlockHeight, HeaderHash, Peer, Tip};
use amaru_pure_stage::Instant;

const EWMA_ALPHA: f64 = 0.2;
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

/// Quality aggregates used for ranking.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PeerScores {
    pub header_lag_ewma: Option<Duration>,
    pub block_response_ewma: Option<Duration>,
    pub bandwidth_ewma_bps: Option<f64>,
    pub keepalive_rtt_ewma: Option<Duration>,
    pub fetch_timeouts: u32,
    pub fetch_successes: u32,
    pub last_change: Option<Instant>,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PeerSnapshot {
    pub peer: Peer,
    pub scores: PeerScores,
    pub tips: Vec<BlockClaim>,
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
    /// Wall-clock for selection (reserved for future staleness-aware ranking).
    pub now: Instant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct ParentInfo {
    parent: Option<HeaderHash>,
    height: BlockHeight,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct ClaimMeta {
    height: BlockHeight,
    parent: Option<HeaderHash>,
    kind: ClaimKind,
    at: Instant,
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
struct PeerState {
    tips: BTreeMap<HeaderHash, ClaimMeta>,
    scores: PeerScores,
}

/// Peer performance map (availability + scores). Owned by the performance worker thread.
#[derive(Debug, Default)]
pub struct PeerPerformance {
    parents: BTreeMap<HeaderHash, ParentInfo>,
    direct: BTreeMap<HeaderHash, BTreeMap<Peer, ClaimMeta>>,
    peers: BTreeMap<Peer, PeerState>,
}

impl PeerPerformance {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn apply_intersection(&mut self, peer: Peer, current: Tip, parent: Option<HeaderHash>, at: Instant) {
        self.insert_claim(peer, current.hash(), current.block_height(), parent, ClaimKind::Intersection, at);
    }

    pub fn apply_header_announcement(&mut self, peer: Peer, header: Tip, parent: Option<HeaderHash>, at: Instant) {
        // First announcer records zero lag (bonus); later peers record delay vs first.
        let lag = self
            .first_announced_at_unlocked(&header.hash())
            .map(|(_, first_at)| at.saturating_since(first_at))
            .unwrap_or(Duration::ZERO);
        self.update_header_lag(&peer, lag, at);
        self.insert_claim(peer, header.hash(), header.block_height(), parent, ClaimKind::HeaderAnnouncement, at);
    }

    #[expect(clippy::too_many_arguments)]
    pub fn apply_block_delivery(
        &mut self,
        peer: Peer,
        hash: HeaderHash,
        height: BlockHeight,
        parent: Option<HeaderHash>,
        at: Instant,
        response: Duration,
        bytes: u64,
    ) {
        self.insert_claim(peer.clone(), hash, height, parent, ClaimKind::BlockDelivery, at);
        self.update_block_delivery(&peer, response, bytes, at);
    }

    pub fn apply_fetch_failure(&mut self, peers: &[Peer], at: Instant) {
        for peer in peers {
            let state = self.peers.entry(peer.clone()).or_default();
            state.scores.fetch_timeouts = state.scores.fetch_timeouts.saturating_add(1);
            state.scores.last_change = Some(at);
        }
    }

    pub fn apply_keepalive_rtt(&mut self, peer: Peer, rtt: Duration, at: Instant) {
        let state = self.peers.entry(peer).or_default();
        state.scores.keepalive_rtt_ewma = Some(ewma_duration(state.scores.keepalive_rtt_ewma, rtt));
        state.scores.last_change = Some(at);
    }

    pub fn apply_clear_peer_availability(&mut self, peer: &Peer) {
        if let Some(state) = self.peers.get_mut(peer) {
            state.tips.clear();
        }
        for claimants in self.direct.values_mut() {
            claimants.remove(peer);
        }
        self.direct.retain(|_, claimants| !claimants.is_empty());
    }

    pub fn apply_forget_peer(&mut self, peer: &Peer) {
        self.peers.remove(peer);
        for claimants in self.direct.values_mut() {
            claimants.remove(peer);
        }
        self.direct.retain(|_, claimants| !claimants.is_empty());
    }

    pub fn apply_prune_below(&mut self, min_height: BlockHeight) {
        self.prune_below(min_height);
    }

    pub fn apply_select_peers_for_fetch(&self, params: SelectPeersParams) -> FetchPeerSet {
        self.select_peers_for_fetch(params)
    }

    pub fn apply_peer_covers_fragment(&self, peer: &Peer, need: &[HeaderHash]) -> bool {
        self.peer_covers_fragment(peer, need)
    }

    pub fn apply_first_announced_at(&self, hash: &HeaderHash) -> Option<(Peer, Instant)> {
        self.first_announced_at_unlocked(hash)
    }

    pub fn apply_direct_claimants(&self, hash: &HeaderHash) -> Vec<(Peer, Instant, ClaimKind)> {
        self.direct_claimants(hash)
    }

    /// Rank candidates for churn (worst first). `now` is reserved for future staleness ranking.
    pub fn apply_rank_peers_for_churn(&self, candidates: &[Peer], _now: Instant) -> Vec<(Peer, PeerScores)> {
        self.rank_peers_for_churn(candidates)
    }

    pub fn apply_scores(&self, peer: &Peer) -> PeerScores {
        self.peers.get(peer).map(|s| s.scores.clone()).unwrap_or_default()
    }

    pub fn apply_snapshot(&self, peer: &Peer) -> Option<PeerSnapshot> {
        self.snapshot(peer)
    }

    /// Re-tip a peer after chainsync rollback to `point`.
    pub fn apply_rollback(&mut self, peer: Peer, point: Tip, parent: Option<HeaderHash>, at: Instant) {
        self.record_rollback(peer, point, parent, at);
    }
}

impl PeerPerformance {
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
                claimants.insert(peer.clone(), meta.clone());
            }
        }

        let state = self.peers.entry(peer).or_default();
        self::dominate_tips(&self.parents, &mut state.tips, hash, meta);
    }

    fn first_announced_at_unlocked(&self, hash: &HeaderHash) -> Option<(Peer, Instant)> {
        let claimants = self.direct.get(hash)?;
        claimants.iter().map(|(p, m)| (p.clone(), m.at)).min_by_key(|(_, at)| *at)
    }

    fn direct_claimants(&self, hash: &HeaderHash) -> Vec<(Peer, Instant, ClaimKind)> {
        match self.direct.get(hash) {
            Some(claimants) => claimants.iter().map(|(p, m)| (p.clone(), m.at, m.kind)).collect(),
            None => Vec::new(),
        }
    }

    fn update_header_lag(&mut self, peer: &Peer, lag: Duration, at: Instant) {
        let state = self.peers.entry(peer.clone()).or_default();
        state.scores.header_lag_ewma = Some(ewma_duration(state.scores.header_lag_ewma, lag));
        state.scores.last_change = Some(at);
    }

    fn update_block_delivery(&mut self, peer: &Peer, response: Duration, bytes: u64, at: Instant) {
        let state = self.peers.entry(peer.clone()).or_default();
        state.scores.block_response_ewma = Some(ewma_duration(state.scores.block_response_ewma, response));
        let secs = response.as_secs_f64().max(1e-6);
        let bps = bytes as f64 / secs;
        state.scores.bandwidth_ewma_bps = Some(ewma_f64(state.scores.bandwidth_ewma_bps, bps));
        state.scores.fetch_successes = state.scores.fetch_successes.saturating_add(1);
        state.scores.last_change = Some(at);
    }

    /// Whether the peer can serve every block in `need`.
    ///
    /// Equivalently: can serve the **last** hash of the fragment. A claim on (or above) that
    /// point implies all ancestors, so partial-prefix peers are not treated as range-capable —
    /// selecting them would leave blockfetch short of the requested range until timeout.
    fn peer_covers_fragment(&self, peer: &Peer, need: &[HeaderHash]) -> bool {
        let Some(last) = need.last() else {
            return true;
        };
        peer_covers_hash(self, peer, *last)
    }

    fn select_peers_for_fetch(&self, params: SelectPeersParams) -> FetchPeerSet {
        let SelectPeersParams { need, max_peers, now: _ } = params;
        if need.is_empty() || max_peers == 0 {
            return FetchPeerSet { peers: Vec::new(), weak: true };
        }

        let mut ranked: Vec<(f64, Peer)> = Vec::new();
        for peer in self.peers.keys() {
            if !self.peer_covers_fragment(peer, &need) {
                continue;
            }
            let score = rank_score(self.peers.get(peer).map(|s| &s.scores), need.len());
            ranked.push((score, peer.clone()));
        }

        ranked.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.1.cmp(&b.1)));

        if ranked.is_empty() {
            return FetchPeerSet { peers: Vec::new(), weak: true };
        }

        let peers: Vec<Peer> = ranked.into_iter().take(max_peers).map(|(_, p)| p).collect();
        FetchPeerSet { peers, weak: false }
    }

    fn rank_peers_for_churn(&self, candidates: &[Peer]) -> Vec<(Peer, PeerScores)> {
        let mut ranked: Vec<(f64, Peer, PeerScores)> = candidates
            .iter()
            .map(|peer| {
                let scores = self.peers.get(peer).map(|s| s.scores.clone()).unwrap_or_default();
                let badness = churn_badness(&scores);
                (badness, peer.clone(), scores)
            })
            .collect();
        ranked.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.1.cmp(&b.1)));
        ranked.into_iter().map(|(_, p, s)| (p, s)).collect()
    }

    fn snapshot(&self, peer: &Peer) -> Option<PeerSnapshot> {
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
        Some(PeerSnapshot { peer: peer.clone(), scores: state.scores.clone(), tips })
    }

    fn record_rollback(&mut self, peer: Peer, point: Tip, parent: Option<HeaderHash>, at: Instant) {
        let hash = point.hash();
        let height = point.block_height();
        self.parents.entry(hash).or_insert(ParentInfo { parent, height });

        if let Some(state) = self.peers.get_mut(&peer) {
            state.tips.clear();
        }
        self.insert_claim(peer, hash, height, parent, ClaimKind::HeaderAnnouncement, at);
    }

    fn prune_below(&mut self, min_height: BlockHeight) {
        self.parents.retain(|_, info| info.height >= min_height);
        let parents = &self.parents;
        self.direct.retain(|hash, claimants| parents.contains_key(hash) && !claimants.is_empty());
        for state in self.peers.values_mut() {
            state.tips.retain(|hash, meta| meta.height >= min_height && parents.contains_key(hash));
        }
    }
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

fn ewma_duration(prev: Option<Duration>, sample: Duration) -> Duration {
    match prev {
        None => sample,
        Some(p) => {
            let prev_secs = p.as_secs_f64();
            let sample_secs = sample.as_secs_f64();
            let next = (1.0 - EWMA_ALPHA) * prev_secs + EWMA_ALPHA * sample_secs;
            Duration::from_secs_f64(next.max(0.0))
        }
    }
}

fn ewma_f64(prev: Option<f64>, sample: f64) -> f64 {
    match prev {
        None => sample,
        Some(p) => (1.0 - EWMA_ALPHA) * p + EWMA_ALPHA * sample,
    }
}

/// Compute a "goodness" score for fetch ranking: higher is better.
///
/// NOTE: **these are made-up heuristics for now, and may be tuned or replaced with a more
/// principled approach later.**
fn rank_score(scores: Option<&PeerScores>, need_len: usize) -> f64 {
    let scores = scores.cloned().unwrap_or_default();
    let mut score = 0.0;

    let response_penalty = scores.block_response_ewma.map(|d| (d.as_secs_f64() + 1.0).ln()).unwrap_or(0.0);
    score -= 2.0 * response_penalty;

    let lag_penalty = scores.header_lag_ewma.map(|d| (d.as_secs_f64() + 1.0).ln()).unwrap_or(0.0);
    score -= if need_len <= 1 { 3.0 } else { 1.0 } * lag_penalty;

    if need_len >= 5
        && let Some(bps) = scores.bandwidth_ewma_bps
    {
        score += (bps.max(1.0)).ln();
    }

    let attempts = scores.fetch_successes.saturating_add(scores.fetch_timeouts);
    if attempts > 0 {
        let rate = scores.fetch_timeouts as f64 / attempts as f64;
        score -= 5.0 * rate;
    }

    score
}

/// Compute a "badness" score for churn ranking: higher is worse.
///
/// NOTE: **these are made-up heuristics for now, and may be tuned or replaced with a more
/// principled approach later.**
fn churn_badness(scores: &PeerScores) -> f64 {
    let mut bad = 0.0;
    let attempts = scores.fetch_successes.saturating_add(scores.fetch_timeouts);
    if attempts > 0 {
        bad += 10.0 * (scores.fetch_timeouts as f64 / attempts as f64);
    }
    if let Some(lag) = scores.header_lag_ewma {
        bad += (lag.as_secs_f64() + 1.0).ln();
    }
    if let Some(resp) = scores.block_response_ewma {
        bad += (resp.as_secs_f64() + 1.0).ln();
    }
    if let Some(bps) = scores.bandwidth_ewma_bps {
        bad -= (bps.max(1.0)).ln() * 0.1;
    }
    bad
}
