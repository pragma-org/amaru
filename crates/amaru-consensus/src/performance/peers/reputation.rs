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

//! Sticky share flags and lazy-decay connection malus.

use std::time::Duration;

use amaru_kernel::Peer;
use amaru_pure_stage::Instant;

use super::{
    BAN_STUB_GRACE, PeerPerformance, peer_mix::DEFAULT_MALUS_HALF_LIFE, quality::PeerScores, record::PeerState,
};

/// Fallback malus half-life when a peer has no known source (same as mix default).
pub const DEFAULT_PEER_MALUS_HALF_LIFE: Duration = DEFAULT_MALUS_HALF_LIFE;

/// Added to connection malus on each failed outbound connect.
///
/// Large enough that one failure sorts the peer behind every healthy candidate. It does not
/// remove the peer from the dial pool: when no healthier candidate can fill a slot, this peer
/// is still offered.
pub const CONNECT_FAIL_IMPULSE: f64 = 8.0;

/// Added when a peer is marked adversarial (cool-down is separate, in peer selection).
pub const ADVERSARIAL_IMPULSE: f64 = 12.0;

/// Sharing requires evolved malus strictly below this (using the peer’s source half-life).
pub const SHARE_MALUS_THRESHOLD: f64 = 0.05;

/// Share-relevant reputation flags stored on the performance map.
///
/// Origin pools live on the performance map. Listen-address policy stays in peer selection.
/// This type only exposes the reputation flags Performance observes across stages.
///
/// Sharing eligibility also requires evolved connection malus below [`SHARE_MALUS_THRESHOLD`]
/// (see [`PeerPerformance::ok_for_sharing`]); that check needs `now` and is not on this struct.
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
    /// Sticky adversarial marker retained across [`PeerPerformance::mark_adversarial`].
    /// Permanent for **sharing** only; outbound dial after cool-down is allowed (malus soft-deprioritises).
    pub adversarial: bool,
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

fn add_malus_impulse(state: &mut PeerState, impulse: f64, at: Instant, half_life: Duration) {
    let evolved = malus_at(state.malus, state.malus_as_of, at, half_life);
    state.malus = (evolved + impulse).max(0.0);
    state.malus_as_of = Some(at);
}

impl PeerPerformance {
    /// Mark peer adversarial: clear claims, scores, and the share-request row; keep a durable reputation stub.
    ///
    /// Retains `ever_connected`, `failure_count`, and last `advertisable`; sets `adversarial = true`
    /// (sharing only). Adds [`ADVERSARIAL_IMPULSE`] to connection malus. Cool-down remains
    /// peer-selection’s job.
    ///
    /// This is not a generic “forget”: a future erase-without-adversarial path would be a
    /// separate operation.
    pub fn mark_adversarial(&mut self, peer: &Peer, at: Instant) {
        self.clear_peer_claims(peer);
        self.share_requests.remove(peer);
        let half_life = self.half_life_for(peer);
        {
            let state = self.peers.entry(*peer).or_default();
            state.tips.clear();
            state.scores = PeerScores::default();
            state.adversarial = true;
            state.stub_until = Some(at + BAN_STUB_GRACE);
            add_malus_impulse(state, ADVERSARIAL_IMPULSE, at, half_life);
        }
        self.note_activity(*peer, at);
    }

    /// Record latest handshake peer-sharing willingness (overwrites prior value).
    ///
    /// Marks the peer as ever-connected (successful handshake). Connection-failure upserts do
    /// not set that flag.
    pub fn record_advertisability(&mut self, peer: Peer, advertisable: bool, at: Instant) {
        {
            let state = self.peers.entry(peer).or_default();
            state.ever_connected = true;
            state.advertisable = advertisable;
            state.scores.last_change = Some(at);
        }
        self.note_activity(peer, at);
    }

    /// Increment connection/protocol failure count and raise connection malus.
    ///
    /// Upserts a reputation stub when needed, but does **not** set `ever_connected`.
    /// The write bumps the peer-view generation, so the next selection query is not skipped.
    pub fn record_connection_failure(&mut self, peer: Peer, at: Instant) {
        let half_life = self.half_life_for(&peer);
        {
            let state = self.peers.entry(peer).or_default();
            state.failure_count = state.failure_count.saturating_add(1);
            state.scores.last_change = Some(at);
            add_malus_impulse(state, CONNECT_FAIL_IMPULSE, at, half_life);
        }
        self.note_activity(peer, at);
        self.bump_generation();
    }

    pub fn share_flags(&self, peer: &Peer) -> Option<PeerShareFlags> {
        self.peers.get(peer).map(|s| s.share_flags())
    }

    /// Whether this peer has a Performance record and passes share reputation checks.
    ///
    /// Requires a successful handshake (`ever_connected`), advertisable willingness, not
    /// sticky-adversarial, and connection malus below [`SHARE_MALUS_THRESHOLD`] after decay with
    /// the peer’s source half-life. The share pool excludes ledger and snapshot origins, and peers
    /// that are neither static nor shared.
    pub fn ok_for_sharing(&self, peer: &Peer, now: Instant) -> bool {
        let Some(state) = self.peers.get(peer) else {
            return false;
        };
        if !state.ever_connected || !state.advertisable || state.adversarial {
            return false;
        }
        let malus = malus_at(state.malus, state.malus_as_of, now, self.half_life_for(peer));
        malus < SHARE_MALUS_THRESHOLD
    }
}
