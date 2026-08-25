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

//! Peer availability claims, quality scores, and outbound peer-source pools.
//!
//! Candidate sources (`static` / `shared` / `snapshot` / `ledger`) and the admin [`PeerMix`]
//! live here so connection malus can always evolve with the peer’s source half-life, and so
//! large peer sets are not copied through pure-stage messages/traces (EDR-031).

use std::{
    collections::{BTreeMap, BTreeSet},
    net::SocketAddr,
    time::Duration,
};

use amaru_kernel::{BlockHeight, HeaderHash, Peer, Point};
use amaru_observability::warn;
use amaru_pure_stage::Instant;
use rand::{Rng, SeedableRng, rngs::StdRng};

use super::peer_mix::{DEFAULT_MALUS_HALF_LIFE, PeerMix, PeerSource};

const EWMA_ALPHA: f64 = 0.2;
/// Safety cap on parent walks. With height-aware early exit this is rarely approached;
/// it only bounds pathological maps that lack usable height information.
const MAX_PARENT_WALK: usize = 512;

/// Fallback malus half-life when a peer has no known source (same as mix default).
pub const DEFAULT_PEER_MALUS_HALF_LIFE: Duration = DEFAULT_MALUS_HALF_LIFE;
/// Added to connection malus on outbound connect exhaustion.
pub const CONNECT_FAIL_IMPULSE: f64 = 1.0;
/// Added when a peer is marked adversarial (cool-down is separate, in peer selection).
pub const ADVERSARIAL_IMPULSE: f64 = 12.0;
/// Sharing requires evolved malus strictly below this (using the peer’s source half-life).
pub const SHARE_MALUS_THRESHOLD: f64 = 0.05;
/// Scale of malus in outbound score: `goodness - λ * malus`.
pub const OUTBOUND_MALUS_LAMBDA: f64 = 1.0;
/// Softmax temperature for weighted outbound sampling.
pub const OUTBOUND_PICK_TEMPERATURE: f64 = 1.0;
/// Bonus for peers with no Performance observation yet (never connected / unknown).
pub const NEVER_CONNECTED_BONUS: f64 = 0.5;
/// Upper bound on peers returned in one share response.
pub const SHARE_POLICY_MAX: u8 = 10;

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

/// Share-relevant reputation flags stored on the performance map.
///
/// Origin (ledger / snapshot / static) and listen-address policy live in peer selection;
/// this type only exposes what Performance observes across stages.
///
/// Sharing eligibility also requires evolved connection malus below [`SHARE_MALUS_THRESHOLD`]
/// (see [`PeerPerformance::apply_ok_for_sharing`]); that check needs `now` and is not on this struct.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PeerShareFlags {
    /// Whether a successful connection (handshake) was ever established with this peer.
    ///
    /// Distinct from map presence: connection failures upsert a reputation stub without
    /// setting this flag.
    pub ever_connected: bool,
    /// Latest handshake peer-sharing willingness (`peer_sharing == 1`).
    pub advertisable: bool,
    /// Lifetime connection/protocol failure counter (telemetry; policy uses malus).
    pub failure_count: u32,
    /// Sticky adversarial marker retained across [`PeerPerformance::apply_peer_adversarial`].
    /// Permanent for **sharing** only; outbound dial after cool-down is allowed (malus soft-deprioritises).
    pub adversarial: bool,
}

/// Result of ingesting addresses learned via peer-sharing.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SharedIngestResult {
    pub added: usize,
    pub total: usize,
}

/// Sizes of the outbound candidate source pools owned by Performance.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SourceCounts {
    pub static_peers: usize,
    pub shared_peers: usize,
    pub snapshot_candidates: usize,
    pub ledger_candidates: usize,
}

/// Parameters for mix + quality outbound selection.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SelectOutboundParams {
    /// How many new outbound dials to return (typically `target − |outbound|`).
    pub open: usize,
    /// Peers that must not be dialed (already outbound and/or cooling down).
    pub excluded: BTreeSet<Peer>,
    /// Deterministic RNG seed from peer selection’s random effect.
    pub seed: [u8; 32],
    pub now: Instant,
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
    /// Sticky once a successful handshake is observed; not set by connection-failure upserts.
    ever_connected: bool,
    /// Latest handshake peer-sharing willingness; latest successful handshake wins.
    advertisable: bool,
    /// Connection / protocol failures (distinct from blockfetch `fetch_timeouts`); telemetry.
    failure_count: u32,
    /// Sticky once set by [`PeerPerformance::apply_peer_adversarial`]; not cleared by clear/availability.
    /// Permanent for peer-sharing filters only.
    adversarial: bool,
    /// Connection-reputation malus at [`Self::malus_as_of`] (lazy exponential decay).
    malus: f64,
    /// Instant when [`Self::malus`] was last evolved for storage.
    malus_as_of: Option<Instant>,
}

impl PeerState {
    fn share_flags(&self) -> PeerShareFlags {
        PeerShareFlags {
            ever_connected: self.ever_connected,
            advertisable: self.advertisable,
            failure_count: self.failure_count,
            adversarial: self.adversarial,
        }
    }
}

/// Peer performance map (availability + scores + source pools). Owned by the performance worker.
#[derive(Debug, Default)]
pub struct PeerPerformance {
    /// header tree link edges
    parents: BTreeMap<HeaderHash, ParentInfo>,
    /// announcements and deliveries by peer, per hash
    direct: BTreeMap<HeaderHash, BTreeMap<Peer, ClaimMeta>>,
    /// announcements by peer, with EWMA scores and tip claims
    peers: BTreeMap<Peer, PeerState>,
    /// Admin mix formula (floors, weights, per-source malus half-lives).
    peer_mix: PeerMix,
    static_peers: BTreeSet<Peer>,
    shared_peers: BTreeSet<Peer>,
    snapshot_candidates: BTreeSet<Peer>,
    ledger_candidates: BTreeSet<Peer>,
}

impl PeerPerformance {
    pub fn new() -> Self {
        Self::default()
    }

    /// Bootstrap candidate pools and the admin mix (typically once at node start).
    pub fn with_sources(
        static_peers: BTreeSet<Peer>,
        snapshot_candidates: BTreeSet<Peer>,
        ledger_candidates: BTreeSet<Peer>,
        peer_mix: PeerMix,
    ) -> Self {
        Self { static_peers, snapshot_candidates, ledger_candidates, peer_mix, ..Self::default() }
    }

    pub fn apply_set_ledger_candidates(&mut self, candidates: BTreeSet<Peer>) {
        self.ledger_candidates = candidates;
    }

    /// Insert peers learned from a share reply (skips other origins and the donor).
    pub fn apply_ingest_shared_peers(&mut self, from: &Peer, addrs: &[SocketAddr]) -> SharedIngestResult {
        let mut added = 0usize;
        for addr in addrs {
            let peer = match Peer::try_from(addr) {
                Ok(peer) => peer,
                Err(reason) => {
                    warn!(
                        protocols::peer_selection::peer::ADDRESS_REJECTED,
                        address = addr.to_string(),
                        reason = reason.to_string()
                    );
                    continue;
                }
            };
            if &peer == from
                || self.static_peers.contains(&peer)
                || self.snapshot_candidates.contains(&peer)
                || self.ledger_candidates.contains(&peer)
            {
                continue;
            }
            if self.shared_peers.insert(peer) {
                added += 1;
            }
        }
        SharedIngestResult { added, total: self.shared_peers.len() }
    }

    pub fn apply_is_static_peer(&self, peer: &Peer) -> bool {
        self.static_peers.contains(peer)
    }

    pub fn apply_static_peers(&self) -> BTreeSet<Peer> {
        self.static_peers.clone()
    }

    pub fn apply_shared_contains(&self, peer: &Peer) -> bool {
        self.shared_peers.contains(peer)
    }

    pub fn apply_source_counts(&self) -> SourceCounts {
        SourceCounts {
            static_peers: self.static_peers.len(),
            shared_peers: self.shared_peers.len(),
            snapshot_candidates: self.snapshot_candidates.len(),
            ledger_candidates: self.ledger_candidates.len(),
        }
    }

    /// Canonical origin: static > shared > snapshot > ledger.
    pub fn canonical_source(&self, peer: &Peer) -> Option<PeerSource> {
        if self.static_peers.contains(peer) {
            Some(PeerSource::Static)
        } else if self.shared_peers.contains(peer) {
            Some(PeerSource::Shared)
        } else if self.snapshot_candidates.contains(peer) {
            Some(PeerSource::Snapshot)
        } else if self.ledger_candidates.contains(peer) {
            Some(PeerSource::Ledger)
        } else {
            None
        }
    }

    /// Half-life for this peer’s source from the mix formula (or global default).
    pub fn half_life_for(&self, peer: &Peer) -> Duration {
        let Some(source) = self.canonical_source(peer) else {
            return DEFAULT_PEER_MALUS_HALF_LIFE;
        };
        self.peer_mix
            .entries()
            .iter()
            .find(|e| e.source == source)
            .map(|e| e.half_life)
            .unwrap_or(DEFAULT_PEER_MALUS_HALF_LIFE)
    }

    /// Mix allotment + quality-weighted sample of peers to dial.
    pub fn apply_select_outbound(&self, params: SelectOutboundParams) -> Vec<Peer> {
        if params.open == 0 {
            return Vec::new();
        }
        let mut eligible_counts = BTreeMap::new();
        let mut eligible_by_source: BTreeMap<PeerSource, Vec<Peer>> = BTreeMap::new();
        for entry in self.peer_mix.entries() {
            let list = self.eligible_for_source(entry.source, &params.excluded);
            eligible_counts.insert(entry.source, list.len());
            eligible_by_source.insert(entry.source, list);
        }
        let allotment = self.peer_mix.allot(params.open, &eligible_counts);
        if allotment.values().all(|&n| n == 0) {
            return Vec::new();
        }

        let mut rng = StdRng::from_seed(params.seed);
        let mut picked = Vec::new();
        let mut already: BTreeSet<Peer> = BTreeSet::new();

        for entry in self.peer_mix.entries() {
            let n = allotment.get(&entry.source).copied().unwrap_or(0);
            if n == 0 {
                continue;
            }
            let candidates = eligible_by_source.remove(&entry.source).unwrap_or_default();
            let candidates: Vec<Peer> = candidates.into_iter().filter(|p| !already.contains(p)).collect();
            if candidates.is_empty() {
                continue;
            }
            let weights = self.outbound_weights_for(&candidates, params.now);
            let items: Vec<(Peer, f64)> = weights.into_iter().map(|w| (w.peer, w.weight)).collect();
            for peer in weighted_sample_without_replacement(&mut rng, items, n) {
                already.insert(peer);
                picked.push(peer);
            }
        }
        picked
    }

    /// Addresses to advertise in a share reply (origin filter + sticky sample + reputation).
    pub fn apply_select_share_peers(&self, requester: &Peer, amount: u8, now: Instant) -> Vec<SocketAddr> {
        let n = (amount.min(SHARE_POLICY_MAX)) as usize;
        if n == 0 {
            return Vec::new();
        }
        let mut eligible = Vec::new();
        for peer in self.share_candidate_pool() {
            if &peer == requester {
                continue;
            }
            let addr = SocketAddr::from(peer);
            // Pool members with no Performance row are treated as shareable (not observed bad).
            // Observed peers must pass advertisability / malus / non-adversarial checks.
            if self.peers.contains_key(&peer) && !self.apply_ok_for_sharing(&peer, now) {
                continue;
            }
            eligible.push((peer, addr));
        }
        eligible.sort_by_key(|a| a.0);
        let seed = {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            requester.hash(&mut hasher);
            hasher.finish()
        };
        let mut rng = StdRng::seed_from_u64(seed);
        use rand::seq::SliceRandom;
        eligible.shuffle(&mut rng);
        eligible.into_iter().take(n).map(|(_, addr)| addr).collect()
    }

    fn share_candidate_pool(&self) -> BTreeSet<Peer> {
        let mut pool = BTreeSet::new();
        for p in &self.static_peers {
            pool.insert(*p);
        }
        for p in self.peers.keys() {
            // Outbound peers that are not ledger/snapshot-derived appear via canonical static/shared only;
            // also include any peer we have scores for that is static or shared.
            if self.canonical_source(p) == Some(PeerSource::Static)
                || self.canonical_source(p) == Some(PeerSource::Shared)
            {
                pool.insert(*p);
            }
        }
        for p in &self.shared_peers {
            pool.insert(*p);
        }
        pool.retain(|p| !self.ledger_candidates.contains(p) && !self.snapshot_candidates.contains(p));
        pool
    }

    fn eligible_for_source(&self, source: PeerSource, excluded: &BTreeSet<Peer>) -> Vec<Peer> {
        let pool: &BTreeSet<Peer> = match source {
            PeerSource::Static => &self.static_peers,
            PeerSource::Shared => &self.shared_peers,
            PeerSource::Snapshot => &self.snapshot_candidates,
            PeerSource::Ledger => &self.ledger_candidates,
        };
        pool.iter().filter(|p| self.canonical_source(p) == Some(source) && !excluded.contains(*p)).cloned().collect()
    }

    fn outbound_weights_for(&self, candidates: &[Peer], now: Instant) -> Vec<OutboundWeight> {
        candidates
            .iter()
            .map(|peer| {
                let half_life = self.half_life_for(peer);
                let (malus, goodness, never_connected) = match self.peers.get(peer) {
                    None => (0.0, 0.0, true),
                    Some(state) => {
                        let malus = malus_at(state.malus, state.malus_as_of, now, half_life);
                        // Fresh / unknown to Performance: no successful handshake yet and no scores
                        // or tip activity. Failure-only stubs set `last_change` (and malus) so they
                        // do not receive the never-connected exploration bonus.
                        let never_connected =
                            !state.ever_connected && state.scores.last_change.is_none() && state.tips.is_empty();
                        let goodness = outbound_goodness(Some(&state.scores));
                        (malus, goodness, never_connected)
                    }
                };
                let mut score = goodness - OUTBOUND_MALUS_LAMBDA * malus;
                if never_connected {
                    score += NEVER_CONNECTED_BONUS;
                }
                OutboundWeight { peer: *peer, weight: outbound_sampling_weight(score), malus }
            })
            .collect()
    }
}

struct OutboundWeight {
    peer: Peer,
    weight: f64,
    #[allow(dead_code)]
    malus: f64,
}

impl PeerPerformance {
    pub fn apply_intersection(&mut self, peer: Peer, current: Point, parent: Option<HeaderHash>, at: Instant) {
        self.insert_claim(peer, current.hash(), current.block_height(), parent, ClaimKind::Intersection, at);
    }

    pub fn apply_header_announcement(&mut self, peer: Peer, header: Point, parent: Option<HeaderHash>, at: Instant) {
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
        self.insert_claim(peer, hash, height, parent, ClaimKind::BlockDelivery, at);
        self.update_block_delivery(&peer, response, bytes, at);
    }

    pub fn apply_fetch_failure(&mut self, peers: &[Peer], at: Instant) {
        for &peer in peers {
            let state = self.peers.entry(peer).or_default();
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

    /// Mark peer adversarial: clear claims and scores, keep a durable reputation stub.
    ///
    /// Retains `ever_connected`, `failure_count`, and last `advertisable`; sets `adversarial = true`
    /// (sharing only). Adds [`ADVERSARIAL_IMPULSE`] to connection malus. Cool-down remains
    /// peer-selection’s job.
    ///
    /// This is not a generic “forget”: a future erase-without-adversarial path would be a
    /// separate operation.
    pub fn apply_peer_adversarial(&mut self, peer: &Peer, at: Instant) {
        for claimants in self.direct.values_mut() {
            claimants.remove(peer);
        }
        self.direct.retain(|_, claimants| !claimants.is_empty());

        let half_life = self.half_life_for(peer);
        let state = self.peers.entry(*peer).or_default();
        state.tips.clear();
        state.scores = PeerScores::default();
        state.adversarial = true;
        apply_malus_impulse(state, ADVERSARIAL_IMPULSE, at, half_life);
    }

    /// Record latest handshake peer-sharing willingness (overwrites prior value).
    ///
    /// Marks the peer as ever-connected (successful handshake). Connection-failure upserts do
    /// not set that flag.
    pub fn apply_advertisability(&mut self, peer: Peer, advertisable: bool, at: Instant) {
        let state = self.peers.entry(peer).or_default();
        state.ever_connected = true;
        state.advertisable = advertisable;
        state.scores.last_change = Some(at);
    }

    /// Increment connection/protocol failure count and raise connection malus.
    ///
    /// Upserts a reputation stub when needed, but does **not** set `ever_connected`.
    pub fn apply_connection_failure(&mut self, peer: Peer, at: Instant) {
        let half_life = self.half_life_for(&peer);
        let state = self.peers.entry(peer).or_default();
        state.failure_count = state.failure_count.saturating_add(1);
        state.scores.last_change = Some(at);
        apply_malus_impulse(state, CONNECT_FAIL_IMPULSE, at, half_life);
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

    pub fn apply_share_flags(&self, peer: &Peer) -> Option<PeerShareFlags> {
        self.peers.get(peer).map(|s| s.share_flags())
    }

    pub fn apply_snapshot(&self, peer: &Peer) -> Option<PeerSnapshot> {
        self.snapshot(peer)
    }

    /// Whether this peer has a Performance record and passes share reputation checks.
    ///
    /// Requires a successful handshake (`ever_connected`), advertisable willingness, not
    /// sticky-adversarial, and connection malus below [`SHARE_MALUS_THRESHOLD`] after decay with
    /// the peer’s source half-life. Peer selection still excludes ledger/snapshot origins and pure
    /// inbound addresses.
    pub fn apply_ok_for_sharing(&self, peer: &Peer, now: Instant) -> bool {
        let Some(state) = self.peers.get(peer) else {
            return false;
        };
        if !state.ever_connected || !state.advertisable || state.adversarial {
            return false;
        }
        let malus = malus_at(state.malus, state.malus_as_of, now, self.half_life_for(peer));
        malus < SHARE_MALUS_THRESHOLD
    }

    /// Re-tip a peer after chainsync rollback to `point`.
    pub fn apply_rollback(&mut self, peer: Peer, point: Point, parent: Option<HeaderHash>, at: Instant) {
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
                claimants.insert(peer, meta.clone());
            }
        }

        let state = self.peers.entry(peer).or_default();
        self::dominate_tips(&self.parents, &mut state.tips, hash, meta);
    }

    fn first_announced_at_unlocked(&self, hash: &HeaderHash) -> Option<(Peer, Instant)> {
        let claimants = self.direct.get(hash)?;
        claimants.iter().map(|(p, m)| (*p, m.at)).min_by_key(|(_, at)| *at)
    }

    fn direct_claimants(&self, hash: &HeaderHash) -> Vec<(Peer, Instant, ClaimKind)> {
        match self.direct.get(hash) {
            Some(claimants) => claimants.iter().map(|(p, m)| (*p, m.at, m.kind)).collect(),
            None => Vec::new(),
        }
    }

    fn update_header_lag(&mut self, peer: &Peer, lag: Duration, at: Instant) {
        let state = self.peers.entry(*peer).or_default();
        state.scores.header_lag_ewma = Some(ewma_duration(state.scores.header_lag_ewma, lag));
        state.scores.last_change = Some(at);
    }

    fn update_block_delivery(&mut self, peer: &Peer, response: Duration, bytes: u64, at: Instant) {
        let state = self.peers.entry(*peer).or_default();
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
        for &peer in self.peers.keys() {
            if !self.peer_covers_fragment(&peer, &need) {
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

    fn rank_peers_for_churn(&self, candidates: &[Peer]) -> Vec<(Peer, PeerScores)> {
        let mut ranked: Vec<(f64, Peer, PeerScores)> = candidates
            .iter()
            .map(|peer| {
                let scores = self.peers.get(peer).map(|s| s.scores.clone()).unwrap_or_default();
                let badness = churn_badness(&scores);
                (badness, *peer, scores)
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
        Some(PeerSnapshot { peer: *peer, scores: state.scores.clone(), tips, share: state.share_flags() })
    }

    fn record_rollback(&mut self, peer: Peer, point: Point, parent: Option<HeaderHash>, at: Instant) {
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

/// Evolve a stored malus charge to `now` with half-life `half_life`.
pub fn malus_at(malus: f64, as_of: Option<Instant>, now: Instant, half_life: Duration) -> f64 {
    if malus <= 0.0 || !malus.is_finite() {
        return 0.0;
    }
    let Some(as_of) = as_of else {
        return malus;
    };
    if half_life.is_zero() {
        return malus;
    }
    let dt = now.saturating_since(as_of);
    if dt.is_zero() {
        return malus;
    }
    let hl = half_life.as_secs_f64().max(f64::EPSILON);
    let factor = 0.5_f64.powf(dt.as_secs_f64() / hl);
    let next = malus * factor;
    if next.is_finite() { next } else { 0.0 }
}

fn apply_malus_impulse(state: &mut PeerState, impulse: f64, at: Instant, half_life: Duration) {
    let evolved = malus_at(state.malus, state.malus_as_of, at, half_life);
    state.malus = (evolved + impulse).max(0.0);
    state.malus_as_of = Some(at);
}

fn weighted_sample_without_replacement(rng: &mut StdRng, mut items: Vec<(Peer, f64)>, n: usize) -> Vec<Peer> {
    let mut out = Vec::with_capacity(n.min(items.len()));
    for _ in 0..n {
        if items.is_empty() {
            break;
        }
        let total: f64 = items.iter().map(|(_, w)| *w).filter(|w| w.is_finite() && *w > 0.0).sum();
        let idx = if total <= 0.0 || !total.is_finite() {
            rng.random_range(0..items.len())
        } else {
            let mut r = rng.random::<f64>() * total;
            let mut chosen = items.len() - 1;
            for (i, (_, w)) in items.iter().enumerate() {
                if *w <= 0.0 || !w.is_finite() {
                    continue;
                }
                r -= w;
                if r <= 0.0 {
                    chosen = i;
                    break;
                }
            }
            chosen
        };
        out.push(items.swap_remove(idx).0);
    }
    out
}

fn outbound_goodness(scores: Option<&PeerScores>) -> f64 {
    // Reuse fetch-oriented heuristics with need_len=1 (no fragment context for dial).
    rank_score(scores, 1)
}

fn outbound_sampling_weight(score: f64) -> f64 {
    let t = OUTBOUND_PICK_TEMPERATURE.max(f64::EPSILON);
    let w = (score / t).exp();
    if w.is_finite() && w > 0.0 { w } else { f64::EPSILON }
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
