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

//! Outbound candidate pools (static, shared, snapshot, ledger) and dial-origin memory.

use std::{collections::BTreeSet, net::SocketAddr, time::Duration};

use amaru_kernel::{Peer, PeerCandidate};
use amaru_observability::warn;
use amaru_pure_stage::Instant;

use super::{PeerPerformance, peer_mix::PeerSource, reputation::DEFAULT_PEER_MALUS_HALF_LIFE};

/// Result of ingesting addresses learned via peer-sharing.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SharedIngestResult {
    pub added: usize,
    /// New addresses refused because [`super::SHARED_PEERS_CAP`] was already reached.
    pub dropped: usize,
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

impl PeerPerformance {
    pub fn set_ledger_candidates(&mut self, candidates: BTreeSet<PeerCandidate>) {
        self.ledger_candidates = candidates;
        self.bump_generation();
    }

    /// Insert peers learned from a share reply (skips other origins and the donor).
    ///
    /// Once [`super::SHARED_PEERS_CAP`] is reached, further newcomers are counted in `dropped`
    /// and the addresses already learned stay. A repeat does not refresh the learned instant.
    pub fn ingest_shared_peers(&mut self, from: &Peer, addrs: &[SocketAddr], at: Instant) -> SharedIngestResult {
        let mut added = 0usize;
        let mut dropped = 0usize;
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
            let candidate = PeerCandidate::from(peer);
            if &peer == from
                || self.static_peers.contains(&candidate)
                || self.snapshot_candidates.contains(&candidate)
                || self.ledger_candidates.contains(&candidate)
                || self.shared_peers.contains(&candidate)
            {
                continue;
            }
            if self.shared_peers.len() >= super::SHARED_PEERS_CAP {
                dropped += 1;
                continue;
            }
            self.remember_shared(candidate, at);
            added += 1;
        }
        let total = self.shared_peers.len();
        if added > 0 {
            self.bump_generation();
        }
        SharedIngestResult { added, dropped, total }
    }

    pub fn is_static_peer(&self, peer: &Peer) -> bool {
        self.canonical_source(peer) == Some(PeerSource::Static)
    }

    pub fn shared_contains(&self, peer: &Peer) -> bool {
        self.shared_peers.contains(&PeerCandidate::from(*peer))
    }

    pub fn source_counts(&self) -> SourceCounts {
        SourceCounts {
            static_peers: self.static_peers.len(),
            shared_peers: self.shared_peers.len(),
            snapshot_candidates: self.snapshot_candidates.len(),
            ledger_candidates: self.ledger_candidates.len(),
        }
    }

    /// Remember a dial so Host/SRV names keep their source half-life and later picks can score
    /// against the last address, without replacing the candidate in its pool.
    pub fn note_dial(&mut self, origin: PeerSource, candidate: &PeerCandidate, peer: Peer, at: Instant) {
        if let Some(previous) = self.last_peer.insert(candidate.clone(), peer)
            && previous != peer
        {
            let empty = self.last_peer_by_peer.get_mut(&previous).is_some_and(|aliases| {
                aliases.remove(candidate);
                aliases.is_empty()
            });
            if empty {
                self.last_peer_by_peer.remove(&previous);
            }
        }
        self.last_peer_by_peer.entry(peer).or_default().insert(candidate.clone());
        self.note_activity(peer, at);
        match self.peer_origin.get(&peer) {
            Some(existing) if *existing <= origin => {}
            _ => {
                self.peer_origin.insert(peer, origin);
            }
        }
    }

    /// Canonical origin: static > shared > snapshot > ledger.
    pub fn canonical_source(&self, peer: &Peer) -> Option<PeerSource> {
        let socket = PeerCandidate::from(*peer);
        if self.static_peers.contains(&socket) {
            Some(PeerSource::Static)
        } else if self.shared_peers.contains(&socket) {
            Some(PeerSource::Shared)
        } else if self.snapshot_candidates.contains(&socket) {
            Some(PeerSource::Snapshot)
        } else if self.ledger_candidates.contains(&socket) {
            Some(PeerSource::Ledger)
        } else {
            self.peer_origin.get(peer).copied()
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

    pub(super) fn share_candidate_pool(&self) -> BTreeSet<Peer> {
        let mut pool = BTreeSet::new();
        pool.extend(self.static_peers.iter().filter_map(PeerCandidate::as_peer));
        pool.extend(self.shared_peers.iter().filter_map(PeerCandidate::as_peer));
        for p in self.peers.keys() {
            if self.canonical_source(p) == Some(PeerSource::Static)
                || self.canonical_source(p) == Some(PeerSource::Shared)
            {
                pool.insert(*p);
            }
        }
        pool.retain(|p| {
            let socket = PeerCandidate::from(*p);
            !self.ledger_candidates.contains(&socket) && !self.snapshot_candidates.contains(&socket)
        });
        pool
    }

    pub(super) fn eligible_for_source(
        &self,
        source: PeerSource,
        excluded: &BTreeSet<PeerCandidate>,
    ) -> Vec<PeerCandidate> {
        let pool = match source {
            PeerSource::Static => &self.static_peers,
            PeerSource::Shared => &self.shared_peers,
            PeerSource::Snapshot => &self.snapshot_candidates,
            PeerSource::Ledger => &self.ledger_candidates,
            PeerSource::Inbound => return Vec::new(),
        };
        pool.iter().filter(|c| !excluded.contains(*c)).cloned().collect()
    }
}
