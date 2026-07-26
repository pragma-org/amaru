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

//! Shared performance resource: peer quality / availability and header lifecycle timings.
//!
//! Stages record **events** via external effects constructed on [`Performance`], e.g.
//! `eff.external(Performance::record_header_announcement(...)).await`.
//! OTel events and metrics are emitted inside the resource when terminal outcomes are recorded.
//!
//! Unit tests may call the `apply_*` methods on a local [`Performance`] directly.

mod effects;
mod header;
mod peer;

use std::{sync::Arc, time::Duration};

use amaru_kernel::{BlockHeight, HeaderHash, Peer, Tip};
use amaru_metrics::Meter;
use amaru_pure_stage::Instant;
pub use effects::*;
pub use header::{ForkSwitchOutcome, HeaderLifecycleOutcome, HeaderPerformance};
pub use peer::{BlockClaim, ClaimKind, FetchPeerSet, PeerPerformance, PeerScores, PeerSnapshot, SelectPeersParams};

/// Resource type installed in pure-stage `Resources`.
pub type ResourcePerformance = Arc<Performance>;

/// Combined peer + header performance state, updated by recording network events.
#[derive(Debug, Default)]
pub struct Performance {
    peers: PeerPerformance,
    headers: HeaderPerformance,
}

impl Performance {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn peers(&self) -> &PeerPerformance {
        &self.peers
    }

    pub fn headers(&self) -> &HeaderPerformance {
        &self.headers
    }

    // --- Event application (also used by effects) ---

    pub fn apply_intersection(&self, peer: Peer, current: Tip, parent: Option<HeaderHash>, at: Instant) {
        self.peers.apply_intersection(peer, current, parent, at);
    }

    pub fn apply_header_announcement(&self, peer: Peer, header: Tip, parent: Option<HeaderHash>, at: Instant) {
        self.peers.apply_header_announcement(peer, header, parent, at);
        self.headers.apply_header_received(header, at);
    }

    pub fn apply_blocks_requested(&self, hashes: &[HeaderHash], requested_at: Instant) {
        self.headers.apply_blocks_requested(hashes, requested_at);
    }

    #[expect(clippy::too_many_arguments)]
    pub fn apply_block_delivery(
        &self,
        peer: Peer,
        hash: HeaderHash,
        height: BlockHeight,
        parent: Option<HeaderHash>,
        at: Instant,
        response: Duration,
        bytes: u64,
    ) {
        self.peers.apply_block_delivery(peer, hash, height, parent, at, response, bytes);
        self.headers.apply_block_downloaded(&hash, at);
    }

    pub fn apply_fetch_failure(&self, peers: &[Peer], at: Instant) {
        self.peers.apply_fetch_failure(peers, at);
    }

    pub fn apply_keepalive_rtt(&self, peer: Peer, rtt: Duration, at: Instant) {
        self.peers.apply_keepalive_rtt(peer, rtt, at);
    }

    pub fn apply_clear_peer_availability(&self, peer: &Peer) {
        self.peers.apply_clear_peer_availability(peer);
    }

    pub fn apply_forget_peer(&self, peer: &Peer) {
        self.peers.apply_forget_peer(peer);
    }

    pub fn apply_prune_below(&self, min_height: BlockHeight) {
        self.peers.apply_prune_below(min_height);
    }

    pub fn apply_select_peers_for_fetch(&self, params: SelectPeersParams) -> FetchPeerSet {
        self.peers.apply_select_peers_for_fetch(params)
    }

    pub fn apply_peer_covers_fragment(&self, peer: &Peer, need: &[HeaderHash]) -> bool {
        self.peers.apply_peer_covers_fragment(peer, need)
    }

    pub fn apply_first_announced_at(&self, hash: &HeaderHash) -> Option<(Peer, Instant)> {
        self.peers.apply_first_announced_at(hash)
    }

    pub fn apply_direct_claimants(&self, hash: &HeaderHash) -> Vec<(Peer, Instant, ClaimKind)> {
        self.peers.apply_direct_claimants(hash)
    }

    pub fn apply_rank_peers_for_churn(&self, candidates: &[Peer], now: Instant) -> Vec<(Peer, PeerScores)> {
        self.peers.apply_rank_peers_for_churn(candidates, now)
    }

    pub fn apply_scores(&self, peer: &Peer) -> PeerScores {
        self.peers.apply_scores(peer)
    }

    pub fn apply_snapshot(&self, peer: &Peer) -> Option<PeerSnapshot> {
        self.peers.apply_snapshot(peer)
    }

    pub fn apply_rollback(&self, peer: Peer, point: Tip, parent: Option<HeaderHash>, at: Instant) {
        self.peers.apply_rollback(peer, point, parent, at);
    }

    pub fn apply_header_rejected(&self, outcome: HeaderLifecycleOutcome, meter: Option<&Meter>) {
        self.headers.apply_header_rejected(outcome, meter);
    }

    pub fn apply_header_abandoned(&self, hash: &HeaderHash, now: Instant, meter: Option<&Meter>) {
        self.headers.apply_header_abandoned(hash, now, meter);
    }

    pub fn apply_fork_started(&self, tip: Tip, started_at: Instant, meter: Option<&Meter>) {
        self.headers.apply_fork_started(tip, started_at, meter);
    }

    pub fn apply_block_valid(&self, hash: &HeaderHash, now: Instant, meter: Option<&Meter>) {
        self.headers.apply_block_valid(hash, now, meter);
    }

    pub fn apply_block_pruned(&self, hash: &HeaderHash, invalid: bool, now: Instant, meter: Option<&Meter>) {
        self.headers.apply_block_pruned(hash, invalid, now, meter);
    }
}

#[cfg(test)]
mod tests;
