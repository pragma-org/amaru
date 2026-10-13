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

//! Retention of per-peer rows and the caps on learned addresses and those rows.
//!
//! Activity is the latest instant a peer was observed. The observers are a handshake, a close, a
//! dial failure, a dial note, a local-use update, a header or block claim, a fetch result, a
//! keep-alive sample, a share request this node answered, a share ingest (the donor), an
//! intersection-not-found mark, and an adversarial mark. The instant only moves forward.
//!
//! A shared address is as fresh as the later of when it was first learned and that activity.
//! Repeating a share reply does not move the learned instant. An in-flight dial is protected by
//! the set peer selection passes in, and the dial note records activity at the dial instant.

#[cfg(test)]
use std::net::SocketAddr;
use std::{collections::BTreeSet, time::Duration};

use amaru_kernel::{Peer, PeerCandidate};
use amaru_pure_stage::Instant;

use super::{PeerPerformance, record::PeerState};

/// How long an untouched, unprotected row is kept.
pub const PEER_RECORD_RETENTION: Duration = Duration::from_secs(24 * 60 * 60);

/// How long a banned peer's reputation stub is kept after the mark.
///
/// While peer selection still lists the peer as protected, each eviction extends this by the
/// same duration, so the stub outlasts the ban.
pub const BAN_STUB_GRACE: Duration = Duration::from_secs(24 * 60 * 60);

/// Learned share-reply addresses. Newcomers past this cap are dropped on ingest.
pub const SHARED_PEERS_CAP: usize = 4096;

/// Per-peer rows (activity index). A live bearer may briefly exceed it; the sweep frees slots.
pub const PEER_RECORD_CAP: usize = 8192;

/// Most entries one eviction inspects. The work stays on one worker operation.
pub const EVICTION_BATCH: usize = 256;

/// What one eviction examined and removed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EvictBatch {
    pub examined: usize,
    pub removed_records: usize,
    pub removed_shared: usize,
}

enum Decide {
    Stop,
    Keep,
    Drop,
}

impl PeerPerformance {
    pub fn peer_record_count(&self) -> usize {
        self.activity.len()
    }

    /// Drop stale unprotected rows, then the oldest unprotected unverified rows while over a cap.
    ///
    /// Examines at most [`EVICTION_BATCH`] entries. When both indexes are non-empty the budget is
    /// split evenly. Each index resumes from its cursor, so a later call does not rescan the
    /// prefix. Does not bump [`Self::generation`]: the next selection round reads the pools.
    pub fn evict_batch(
        &mut self,
        now: Instant,
        protected_peers: &BTreeSet<Peer>,
        protected_candidates: &BTreeSet<PeerCandidate>,
    ) -> EvictBatch {
        let live: BTreeSet<Peer> = self.connections.values().map(|live| live.record.peer).collect();
        let (record_budget, shared_budget) = self.eviction_budgets();
        let (record_examined, removed_records) =
            self.evict_peer_records(now, record_budget, &live, protected_peers, protected_candidates);
        let (shared_examined, removed_shared) =
            self.evict_shared_candidates(now, shared_budget, &live, protected_peers, protected_candidates);
        EvictBatch { examined: record_examined + shared_examined, removed_records, removed_shared }
    }

    pub(super) fn note_activity(&mut self, peer: Peer, at: Instant) {
        if let Some(prev) = self.activity.get(&peer).copied() {
            if at <= prev {
                return;
            }
            self.activity_order.remove(&(prev, peer));
        }
        self.activity.insert(peer, at);
        self.activity_order.insert((at, peer));
    }

    /// Remember a learned address. A repeat does not move the learned instant.
    pub(super) fn remember_shared(&mut self, candidate: PeerCandidate, at: Instant) {
        if self.shared_learned.contains_key(&candidate) {
            return;
        }
        self.shared_peers.insert(candidate.clone());
        self.shared_learned.insert(candidate.clone(), at);
        self.shared_order.insert((at, candidate));
    }

    pub(super) fn clear_peer_claims(&mut self, peer: &Peer) {
        if let Some(state) = self.peers.get_mut(peer) {
            state.tips.clear();
        }
        let Some(hashes) = self.claim_index.remove(peer) else {
            return;
        };
        for hash in hashes {
            let remove_hash = match self.direct.get_mut(&hash) {
                Some(claimants) => {
                    claimants.remove(peer);
                    claimants.is_empty()
                }
                None => false,
            };
            if remove_hash {
                self.direct.remove(&hash);
            }
        }
    }

    fn eviction_budgets(&self) -> (usize, usize) {
        let records = !self.activity_order.is_empty();
        let shared = !self.shared_order.is_empty();
        match (records, shared) {
            (true, true) => (EVICTION_BATCH / 2, EVICTION_BATCH - EVICTION_BATCH / 2),
            (true, false) => (EVICTION_BATCH, 0),
            (false, true) => (0, EVICTION_BATCH),
            (false, false) => (0, 0),
        }
    }

    fn evict_peer_records(
        &mut self,
        now: Instant,
        budget: usize,
        live: &BTreeSet<Peer>,
        protected_peers: &BTreeSet<Peer>,
        protected_candidates: &BTreeSet<PeerCandidate>,
    ) -> (usize, usize) {
        let keys = collect_from(&self.activity_order, self.activity_cursor, budget);
        let mut examined = 0usize;
        let mut removed = 0usize;
        let mut stopped = false;
        for (at, peer) in &keys {
            if self.activity.get(peer) != Some(at) {
                continue;
            }
            examined += 1;
            match self.decide_peer(*peer, *at, now, live, protected_peers, protected_candidates) {
                Decide::Stop => {
                    stopped = true;
                    break;
                }
                Decide::Keep => {}
                Decide::Drop => {
                    self.forget_peer(*peer);
                    removed += 1;
                }
            }
        }
        advance_cursor(&mut self.activity_cursor, &keys, budget, stopped);
        (examined, removed)
    }

    fn evict_shared_candidates(
        &mut self,
        now: Instant,
        budget: usize,
        live: &BTreeSet<Peer>,
        protected_peers: &BTreeSet<Peer>,
        protected_candidates: &BTreeSet<PeerCandidate>,
    ) -> (usize, usize) {
        let keys = collect_from(&self.shared_order, self.shared_cursor.clone(), budget);
        let mut examined = 0usize;
        let mut removed = 0usize;
        for (learned, candidate) in &keys {
            if self.shared_learned.get(candidate) != Some(learned) {
                continue;
            }
            examined += 1;
            if self.drop_shared(candidate, *learned, now, live, protected_peers, protected_candidates) {
                removed += 1;
            }
        }
        advance_cursor(&mut self.shared_cursor, &keys, budget, false);
        (examined, removed)
    }

    fn decide_peer(
        &mut self,
        peer: Peer,
        at: Instant,
        now: Instant,
        live: &BTreeSet<Peer>,
        protected_peers: &BTreeSet<Peer>,
        protected_candidates: &BTreeSet<PeerCandidate>,
    ) -> Decide {
        let stale = now.saturating_since(at) >= PEER_RECORD_RETENTION;
        let over_cap = self.activity.len() > PEER_RECORD_CAP;
        if !stale && !over_cap {
            return Decide::Stop;
        }
        if self.externally_protected(peer, live, protected_peers, protected_candidates) {
            self.refresh_ban_stub(peer, now);
            return Decide::Keep;
        }
        if self.stub_active(peer, now) || (!stale && self.is_established_peer(peer)) {
            return Decide::Keep;
        }
        Decide::Drop
    }

    fn drop_shared(
        &mut self,
        candidate: &PeerCandidate,
        learned: Instant,
        now: Instant,
        live: &BTreeSet<Peer>,
        protected_peers: &BTreeSet<Peer>,
        protected_candidates: &BTreeSet<PeerCandidate>,
    ) -> bool {
        let useful = self.shared_useful_at(candidate, learned);
        let stale = now.saturating_since(useful) >= PEER_RECORD_RETENTION;
        let over_cap = self.shared_peers.len() > SHARED_PEERS_CAP;
        if protected_candidates.contains(candidate) {
            return false;
        }
        if let Some(peer) = candidate.as_peer()
            && (self.externally_protected(peer, live, protected_peers, protected_candidates)
                || self.stub_active(peer, now))
        {
            return false;
        }
        if !stale && !over_cap {
            return false;
        }
        if !stale && self.candidate_is_established(candidate) {
            return false;
        }
        self.forget_shared(candidate, learned);
        true
    }

    fn shared_useful_at(&self, candidate: &PeerCandidate, learned: Instant) -> Instant {
        let Some(peer) = candidate.as_peer() else {
            return learned;
        };
        match self.activity.get(&peer).copied() {
            Some(at) if at > learned => at,
            _ => learned,
        }
    }

    fn externally_protected(
        &self,
        peer: Peer,
        live: &BTreeSet<Peer>,
        protected_peers: &BTreeSet<Peer>,
        protected_candidates: &BTreeSet<PeerCandidate>,
    ) -> bool {
        if live.contains(&peer) || protected_peers.contains(&peer) {
            return true;
        }
        let socket = PeerCandidate::from(peer);
        protected_candidates.contains(&socket)
            || self.static_peers.contains(&socket)
            || self.snapshot_candidates.contains(&socket)
            || self.ledger_candidates.contains(&socket)
    }

    fn stub_active(&self, peer: Peer, now: Instant) -> bool {
        self.peers.get(&peer).and_then(|state| state.stub_until).is_some_and(|until| until > now)
    }

    /// Handshake, a score change, a keep-alive sample, or a fetch success or timeout.
    fn is_established_peer(&self, peer: Peer) -> bool {
        let Some(state) = self.peers.get(&peer) else {
            return false;
        };
        peer_is_established(state)
    }

    fn candidate_is_established(&self, candidate: &PeerCandidate) -> bool {
        candidate.as_peer().is_some_and(|peer| self.is_established_peer(peer))
    }

    fn refresh_ban_stub(&mut self, peer: Peer, now: Instant) {
        let Some(state) = self.peers.get_mut(&peer) else {
            return;
        };
        if !state.adversarial {
            return;
        }
        let until = now + BAN_STUB_GRACE;
        if state.stub_until.is_none_or(|existing| existing < until) {
            state.stub_until = Some(until);
        }
    }

    fn forget_peer(&mut self, peer: Peer) {
        self.clear_peer_claims(&peer);
        self.peers.remove(&peer);
        self.share_requests.remove(&peer);
        self.last_close.remove(&peer);
        self.last_connect_failure.remove(&peer);
        self.last_shared_at.remove(&peer);
        self.uninteresting.remove(&peer);
        self.peer_origin.remove(&peer);
        if let Some(candidates) = self.last_peer_by_peer.remove(&peer) {
            for candidate in candidates {
                if self.last_peer.get(&candidate).copied() == Some(peer) {
                    self.last_peer.remove(&candidate);
                }
            }
        }
        if let Some(at) = self.activity.remove(&peer) {
            self.activity_order.remove(&(at, peer));
        }
    }

    fn forget_shared(&mut self, candidate: &PeerCandidate, learned: Instant) {
        if self.shared_learned.get(candidate) != Some(&learned) {
            self.shared_order.remove(&(learned, candidate.clone()));
            return;
        }
        self.shared_peers.remove(candidate);
        self.shared_learned.remove(candidate);
        self.shared_order.remove(&(learned, candidate.clone()));
    }

    /// Cap-sized maps whose oldest half-batch on each index is stale.
    #[cfg(test)]
    pub fn testing_seed_eviction_cap(&mut self, stale_at: Instant, fresh_at: Instant) {
        let stale_records = EVICTION_BATCH / 2;
        for n in 0..PEER_RECORD_CAP {
            let at = if n < stale_records { stale_at } else { fresh_at };
            self.testing_insert_idle_peer(Peer::for_test((n as u16).saturating_add(1)), at);
        }
        let stale_shared = EVICTION_BATCH / 2;
        for n in 0..SHARED_PEERS_CAP {
            let at = if n < stale_shared { stale_at } else { fresh_at };
            self.testing_insert_shared(addr_from_port(20_000u16.saturating_add(n as u16)), at);
        }
    }

    #[cfg(test)]
    pub fn testing_insert_idle_peer(&mut self, peer: Peer, at: Instant) {
        self.note_activity(peer, at);
    }

    #[cfg(test)]
    pub fn testing_set_activity(&mut self, peer: Peer, at: Instant) {
        if let Some(prev) = self.activity.insert(peer, at) {
            self.activity_order.remove(&(prev, peer));
        }
        self.activity_order.insert((at, peer));
    }

    #[cfg(test)]
    pub fn testing_insert_shared(&mut self, addr: SocketAddr, at: Instant) {
        let Ok(peer) = Peer::try_from(addr) else {
            return;
        };
        let candidate = PeerCandidate::from(peer);
        self.shared_peers.insert(candidate.clone());
        if let Some(prev) = self.shared_learned.insert(candidate.clone(), at) {
            self.shared_order.remove(&(prev, candidate.clone()));
        }
        self.shared_order.insert((at, candidate));
    }

    #[cfg(test)]
    pub fn testing_stub_until(&self, peer: &Peer) -> Option<Instant> {
        self.peers.get(peer).and_then(|state| state.stub_until)
    }
}

#[cfg(test)]
fn addr_from_port(port: u16) -> SocketAddr {
    SocketAddr::from(Peer::for_test(port))
}

fn peer_is_established(state: &PeerState) -> bool {
    state.ever_connected
        || state.scores.last_change.is_some()
        || state.scores.keepalive_rtt_latest.is_some()
        || state.scores.fetch_successes > 0
        || state.scores.fetch_timeouts > 0
}

fn collect_from<K: Clone + Ord>(
    order: &BTreeSet<(Instant, K)>,
    cursor: Option<(Instant, K)>,
    budget: usize,
) -> Vec<(Instant, K)> {
    if budget == 0 {
        return Vec::new();
    }
    let start = match cursor {
        Some(cursor) => std::ops::Bound::Excluded(cursor),
        None => std::ops::Bound::Unbounded,
    };
    order.range((start, std::ops::Bound::Unbounded)).take(budget).cloned().collect()
}

fn advance_cursor<K: Clone>(cursor: &mut Option<(Instant, K)>, keys: &[(Instant, K)], budget: usize, stopped: bool) {
    if stopped || keys.len() < budget {
        *cursor = None;
    } else {
        *cursor = keys.last().cloned();
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeSet, net::SocketAddr, time::Duration};

    use amaru_kernel::{BlockHeight, HeaderHash, Peer, PeerCandidate, Point, Slot};
    use amaru_ouroboros::{ConnectionDirection, ConnectionId, ConnectionRecord, LocalUse, ObservedAt};
    use amaru_pure_stage::Instant;

    use super::{BAN_STUB_GRACE, EVICTION_BATCH, PEER_RECORD_CAP, PEER_RECORD_RETENTION, SHARED_PEERS_CAP};
    use crate::performance::{PeerMix, PeerPerformance};

    fn t(secs: u64) -> Instant {
        Instant::at_offset(Duration::from_secs(secs), Duration::ZERO)
    }

    fn observed(secs: u64) -> ObservedAt {
        ObservedAt::new(Duration::from_secs(secs), Duration::ZERO)
    }

    fn hash(byte: u8) -> HeaderHash {
        HeaderHash::from([byte; 32])
    }

    fn tip(byte: u8, height: u64) -> Point {
        Point::Specific(Slot::from(height), hash(byte), BlockHeight::from(height))
    }

    fn addr(port: u16) -> SocketAddr {
        SocketAddr::from(Peer::for_test(port))
    }

    fn empty() -> (BTreeSet<Peer>, BTreeSet<PeerCandidate>) {
        (BTreeSet::new(), BTreeSet::new())
    }

    fn connect(peers: &mut PeerPerformance, peer: Peer, at: Instant) {
        let observed_at = ObservedAt::new(at.sim_elapsed(), Duration::ZERO);
        peers.record_connection_established(
            ConnectionRecord {
                peer,
                conn_id: ConnectionId::initial(),
                direction: ConnectionDirection::Outbound,
                full_duplex_capable: true,
                full_duplex: false,
                advertisable: true,
                local_use: LocalUse::None,
                established_at: observed_at,
            },
            observed_at,
        );
    }

    #[test]
    fn idle_peer_is_evicted_once_retention_elapses() {
        let mut peers = PeerPerformance::new();
        let who = Peer::for_test(1);
        let (protected_peers, protected_candidates) = empty();
        peers.record_header_announcement(who, tip(1, 1), None, t(1_000));
        peers.record_share_request_served(who, 4, observed(1_000));
        let generation = peers.generation();

        let before = t(1_000) + PEER_RECORD_RETENTION - Duration::from_secs(1);
        let kept = peers.evict_batch(before, &protected_peers, &protected_candidates);
        assert_eq!(kept.removed_records, 0);
        assert!(peers.share_flags(&who).is_some());
        assert_eq!(peers.direct_claimants(&hash(1)).len(), 1);

        let due = t(1_000) + PEER_RECORD_RETENTION;
        let gone = peers.evict_batch(due, &protected_peers, &protected_candidates);
        assert_eq!(gone.removed_records, 1);
        assert!(peers.share_flags(&who).is_none());
        assert!(peers.share_requests(&who).is_none());
        assert!(peers.direct_claimants(&hash(1)).is_empty());
        assert_eq!(peers.peer_record_count(), 0);
        assert_eq!(peers.generation(), generation);
    }

    #[test]
    fn protected_peers_are_kept() {
        let static_peer = Peer::for_test(1);
        let snapshot_peer = Peer::for_test(2);
        let ledger_peer = Peer::for_test(3);
        let live_peer = Peer::for_test(4);
        let banned = Peer::for_test(5);
        let dialing = Peer::for_test(6);
        let idle = Peer::for_test(7);
        let ancient = t(1_000);
        let now = ancient + PEER_RECORD_RETENTION + Duration::from_secs(1);
        let mut peers = PeerPerformance::with_sources(
            BTreeSet::from([PeerCandidate::from(static_peer)]),
            BTreeSet::from([PeerCandidate::from(snapshot_peer)]),
            BTreeSet::from([PeerCandidate::from(ledger_peer)]),
            PeerMix::default(),
        );
        for who in [static_peer, snapshot_peer, ledger_peer, live_peer, banned, dialing, idle] {
            peers.testing_insert_idle_peer(who, ancient);
        }
        connect(&mut peers, live_peer, ancient);
        peers.testing_set_activity(live_peer, ancient);
        peers.mark_adversarial(&banned, ancient);
        peers.testing_set_activity(banned, ancient - Duration::from_secs(48 * 60 * 60));
        let protected_peers = BTreeSet::from([banned]);
        let protected_candidates = BTreeSet::from([PeerCandidate::from(dialing)]);

        let batch = peers.evict_batch(now, &protected_peers, &protected_candidates);
        assert_eq!(batch.removed_records, 1);
        assert!(peers.share_flags(&static_peer).is_none());
        assert_eq!(peers.peer_record_count(), 6);
        for who in [static_peer, snapshot_peer, ledger_peer, live_peer, banned, dialing] {
            assert!(peers.activity_contains_for_test(who), "{who}");
        }
        assert!(!peers.activity_contains_for_test(idle));
        assert_eq!(peers.testing_stub_until(&banned), Some(now + BAN_STUB_GRACE));
        assert!(peers.connection(ConnectionId::initial()).is_some());
    }

    #[test]
    fn banned_stub_is_kept_through_the_ban_and_the_grace() {
        let mut peers = PeerPerformance::new();
        let who = Peer::for_test(8);
        let (protected_peers, protected_candidates) = empty();
        let marked = t(50_000);
        peers.mark_adversarial(&who, marked);
        peers.testing_set_activity(who, marked - Duration::from_secs(48 * 60 * 60));

        let during_ban = marked + Duration::from_secs(10 * 60);
        peers.evict_batch(during_ban, &protected_peers, &protected_candidates);
        assert!(peers.share_flags(&who).is_some_and(|flags| flags.adversarial));

        let almost = marked + BAN_STUB_GRACE - Duration::from_secs(1);
        peers.evict_batch(almost, &protected_peers, &protected_candidates);
        assert!(peers.share_flags(&who).is_some_and(|flags| flags.adversarial));
        assert_eq!(peers.testing_stub_until(&who), Some(marked + BAN_STUB_GRACE));

        let ended = marked + BAN_STUB_GRACE;
        peers.evict_batch(ended, &protected_peers, &protected_candidates);
        assert!(peers.share_flags(&who).is_none());
    }

    #[test]
    fn shared_cap_drops_newcomers_on_ingest_and_the_sweep_drops_the_oldest() {
        let mut peers = PeerPerformance::new();
        let donor = Peer::for_test(9);
        let learned = t(2_000);
        for port in 1..=SHARED_PEERS_CAP as u16 {
            peers.testing_insert_shared(addr(port), learned + Duration::from_secs(u64::from(port)));
        }
        let flood: Vec<SocketAddr> = (30_000..30_100).map(addr).collect();
        let ingested = peers.ingest_shared_peers(&donor, &flood, learned + Duration::from_secs(10_000));
        assert_eq!(ingested.added, 0);
        assert_eq!(ingested.dropped, flood.len());
        assert_eq!(peers.source_counts().shared_peers, SHARED_PEERS_CAP);
        assert!(!peers.shared_contains(&Peer::for_test(30_000)));

        for port in 1..101 {
            peers.testing_insert_shared(addr(40_000 + port), learned);
        }
        assert!(peers.source_counts().shared_peers > SHARED_PEERS_CAP);
        let (protected_peers, protected_candidates) = empty();
        let now = learned + Duration::from_secs(2);
        let mut removed = 0usize;
        let mut steps = 0usize;
        while peers.source_counts().shared_peers > SHARED_PEERS_CAP {
            let batch = peers.evict_batch(now, &protected_peers, &protected_candidates);
            assert!(batch.examined <= EVICTION_BATCH);
            assert!(batch.removed_shared <= EVICTION_BATCH);
            assert!(batch.removed_shared > 0);
            removed += batch.removed_shared;
            steps += 1;
            assert!(steps < 8, "sweep did not return to the cap");
        }
        assert!(removed >= 100);
        assert_eq!(peers.source_counts().shared_peers, SHARED_PEERS_CAP);
        assert!(peers.shared_contains(&Peer::for_test(SHARED_PEERS_CAP as u16)));
        assert!(!peers.shared_contains(&Peer::for_test(40_001)));
    }

    #[test]
    fn peer_record_cap_drops_a_new_share_row_and_the_sweep_drops_idle_rows() {
        let mut peers = PeerPerformance::new();
        let stale = t(3_000);
        for port in 1..=PEER_RECORD_CAP as u16 {
            peers.testing_insert_idle_peer(Peer::for_test(port), stale);
        }
        let newcomer = Peer::for_test(60_000);
        peers.record_share_request_served(newcomer, 2, observed(3_000));
        assert!(peers.share_requests(&newcomer).is_none());
        assert_eq!(peers.peer_record_count(), PEER_RECORD_CAP);

        let known = Peer::for_test(1);
        peers.record_share_request_served(known, 2, observed(3_000));
        assert!(peers.share_requests(&known).is_some());
        assert_eq!(peers.peer_record_count(), PEER_RECORD_CAP);

        let scored = Peer::for_test(60_001);
        let fresh = stale + Duration::from_secs(10);
        peers.record_advertisability(scored, true, fresh);
        peers.record_connection_established(
            ConnectionRecord {
                peer: Peer::for_test(60_002),
                conn_id: ConnectionId::initial(),
                direction: ConnectionDirection::Outbound,
                full_duplex_capable: true,
                full_duplex: false,
                advertisable: false,
                local_use: LocalUse::None,
                established_at: observed(3_010),
            },
            observed(3_010),
        );
        assert!(peers.peer_record_count() > PEER_RECORD_CAP);

        let (protected_peers, protected_candidates) = empty();
        let mut steps = 0usize;
        while peers.peer_record_count() > PEER_RECORD_CAP {
            let batch = peers.evict_batch(fresh, &protected_peers, &protected_candidates);
            assert!(batch.examined <= EVICTION_BATCH);
            assert!(batch.removed_records > 0);
            assert!(batch.removed_records <= EVICTION_BATCH);
            steps += 1;
            assert!(steps < 8);
        }
        assert!(peers.share_flags(&scored).is_some_and(|flags| flags.ever_connected));
        assert!(peers.connection(ConnectionId::initial()).is_some());
        assert!(peers.peer_record_count() <= PEER_RECORD_CAP);
    }

    #[test]
    fn a_share_flood_does_not_evict_scored_peers() {
        let mut peers = PeerPerformance::new();
        let scored = Peer::for_test(7_000);
        let donor = Peer::for_test(7_001);
        let learned = t(4_000);
        let fresh = learned + Duration::from_secs(60);
        peers.record_keepalive_rtt(scored, Duration::from_millis(20), fresh);
        peers.testing_insert_shared(addr(7_000), learned);
        for port in 1..SHARED_PEERS_CAP as u16 {
            peers.testing_insert_shared(addr(port), learned + Duration::from_secs(1));
        }
        assert_eq!(peers.source_counts().shared_peers, SHARED_PEERS_CAP);

        let flood: Vec<SocketAddr> = (50_000..51_000).map(addr).collect();
        let first = peers.ingest_shared_peers(&donor, &flood, fresh);
        assert_eq!(first.added, 0);
        assert_eq!(first.dropped, flood.len());
        let repeated = peers.ingest_shared_peers(&donor, &[addr(1)], fresh + Duration::from_secs(48 * 60 * 60));
        assert_eq!(repeated.added, 0);
        assert_eq!(repeated.dropped, 0);

        for port in 1..50u16 {
            peers.testing_insert_shared(addr(52_000 + port), learned);
        }
        let (protected_peers, protected_candidates) = empty();
        let mut steps = 0usize;
        while peers.source_counts().shared_peers > SHARED_PEERS_CAP {
            let batch = peers.evict_batch(fresh, &protected_peers, &protected_candidates);
            assert!(batch.removed_records == 0, "scored rows must stay");
            assert!(batch.removed_shared > 0);
            steps += 1;
            assert!(steps < 8);
        }
        assert!(peers.shared_contains(&scored));
        assert!(peers.scores(&scored).keepalive_rtt_latest.is_some());
        assert!(!peers.shared_contains(&Peer::for_test(50_000)));
        assert!(!peers.shared_contains(&Peer::for_test(52_001)));

        let due = learned + Duration::from_secs(1) + PEER_RECORD_RETENTION;
        let mut steps = 0usize;
        while peers.shared_contains(&Peer::for_test(1)) {
            peers.evict_batch(due, &protected_peers, &protected_candidates);
            steps += 1;
            assert!(steps < 40, "a repeat ingest must not refresh the learned instant");
        }
        assert!(peers.shared_contains(&scored));
        assert!(peers.scores(&scored).keepalive_rtt_latest.is_some());
    }

    #[test]
    fn one_eviction_examines_at_most_one_batch() {
        let mut peers = PeerPerformance::new();
        let stale = t(5_000);
        let now = stale + PEER_RECORD_RETENTION;
        let (protected_peers, protected_candidates) = empty();
        for port in 1..=400u16 {
            peers.testing_insert_idle_peer(Peer::for_test(port), stale);
        }
        let first = peers.evict_batch(now, &protected_peers, &protected_candidates);
        assert_eq!(first.examined, EVICTION_BATCH);
        assert_eq!(first.removed_records, EVICTION_BATCH);
        assert_eq!(peers.peer_record_count(), 400 - EVICTION_BATCH);

        let second = peers.evict_batch(now, &protected_peers, &protected_candidates);
        assert_eq!(second.removed_records, 400 - EVICTION_BATCH);
        assert!(second.examined <= EVICTION_BATCH);
        assert_eq!(peers.peer_record_count(), 0);
    }

    #[test]
    fn the_cursor_passes_a_protected_prefix() {
        let mut peers = PeerPerformance::new();
        let ancient = t(6_000);
        let now = ancient + PEER_RECORD_RETENTION + Duration::from_secs(10);
        let mut protected_peers = BTreeSet::new();
        for port in 1..=EVICTION_BATCH as u16 {
            let who = Peer::for_test(port);
            peers.testing_insert_idle_peer(who, ancient);
            protected_peers.insert(who);
        }
        for port in 1..=10u16 {
            peers.testing_insert_idle_peer(Peer::for_test(8_000 + port), ancient + Duration::from_secs(1));
        }
        let protected_candidates = BTreeSet::new();
        let first = peers.evict_batch(now, &protected_peers, &protected_candidates);
        assert_eq!(first.removed_records, 0);
        assert_eq!(first.examined, EVICTION_BATCH);
        assert_eq!(peers.peer_record_count(), EVICTION_BATCH + 10);

        let second = peers.evict_batch(now, &protected_peers, &protected_candidates);
        assert_eq!(second.removed_records, 10);
        assert_eq!(peers.peer_record_count(), EVICTION_BATCH);
    }

    impl PeerPerformance {
        fn activity_contains_for_test(&self, peer: Peer) -> bool {
            self.activity.contains_key(&peer)
        }
    }

    #[test]
    fn activity_does_not_move_backward() {
        let mut peers = PeerPerformance::new();
        let who = Peer::for_test(11);
        peers.note_activity(who, t(20));
        peers.note_activity(who, t(10));
        let (protected_peers, protected_candidates) = empty();
        peers.evict_batch(
            t(20) + PEER_RECORD_RETENTION - Duration::from_secs(1),
            &protected_peers,
            &protected_candidates,
        );
        assert_eq!(peers.peer_record_count(), 1);
    }
}
