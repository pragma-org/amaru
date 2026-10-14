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

//! Addresses returned in one peer-sharing reply.
//!
//! The worker copies [`ShareCandidate`] rows. Sorting, shuffling, and the sample run on the
//! caller, using the requester seed from [`share_reply_seed`].

use std::{
    hash::{Hash, Hasher},
    net::SocketAddr,
};

use amaru_kernel::Peer;
pub use amaru_protocols::peer_sharing::SHARE_POLICY_MAX;
use amaru_pure_stage::Instant;
use rand::{SeedableRng, rngs::StdRng, seq::SliceRandom};

use super::PeerPerformance;

/// One pool member copied off the worker for a share reply.
///
/// `shareable` is false when the peer has a performance row and fails the share checks.
/// A pool member with no row is shareable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShareCandidate {
    pub peer: Peer,
    pub address: SocketAddr,
    pub shareable: bool,
}

/// Seed used for a share reply. It depends only on the requester, so the sample does not
/// move when other simulation effects draw randomness.
pub fn share_reply_seed(requester: &Peer) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    requester.hash(&mut hasher);
    hasher.finish()
}

/// Draw a share reply from a candidate snapshot.
///
/// Eligible rows are sorted by peer, shuffled with `seed`, then truncated to `amount`
/// (at most [`SHARE_POLICY_MAX`]). The requester is left out.
pub fn sample_share_peers(requester: &Peer, amount: u8, candidates: &[ShareCandidate], seed: u64) -> Vec<SocketAddr> {
    let n = (amount.min(SHARE_POLICY_MAX)) as usize;
    if n == 0 {
        return Vec::new();
    }
    let mut eligible: Vec<ShareCandidate> =
        candidates.iter().copied().filter(|candidate| candidate.shareable && &candidate.peer != requester).collect();
    eligible.sort_by_key(|candidate| candidate.peer);
    let mut rng = StdRng::seed_from_u64(seed);
    eligible.shuffle(&mut rng);
    eligible.into_iter().take(n).map(|candidate| candidate.address).collect()
}

impl PeerPerformance {
    /// Copy the fields a share reply needs. Does not sort or sample.
    pub fn share_reply_candidates(&self, now: Instant) -> Vec<ShareCandidate> {
        self.share_candidate_pool()
            .into_iter()
            .map(|peer| {
                let shareable = !self.peers.contains_key(&peer) || self.ok_for_sharing(&peer, now);
                ShareCandidate { peer, address: SocketAddr::from(peer), shareable }
            })
            .collect()
    }

    /// In-memory share reply for tests that already hold the map.
    ///
    /// Stage effects copy the candidate rows on the worker and sample after that copy returns.
    pub fn select_share_peers(&self, requester: &Peer, amount: u8, now: Instant) -> Vec<SocketAddr> {
        sample_share_peers(requester, amount, &self.share_reply_candidates(now), share_reply_seed(requester))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(port: u16, shareable: bool) -> ShareCandidate {
        let peer = Peer::for_test(port);
        ShareCandidate { peer, address: SocketAddr::from(peer), shareable }
    }

    #[test]
    fn sample_share_peers_is_unchanged_for_a_fixed_seed() {
        let requester = Peer::for_test(5000);
        let candidates = vec![
            candidate(5001, true),
            candidate(5002, true),
            candidate(5003, false),
            candidate(5004, true),
            candidate(5005, true),
            candidate(5000, true),
        ];
        let seed = 0x51e0_u64;
        let selected = sample_share_peers(&requester, 3, &candidates, seed);
        assert_eq!(selected, sample_share_peers(&requester, 3, &candidates, seed));
        assert_eq!(
            selected,
            vec![
                SocketAddr::from(Peer::for_test(5005)),
                SocketAddr::from(Peer::for_test(5004)),
                SocketAddr::from(Peer::for_test(5002)),
            ]
        );
        assert!(selected.iter().all(|addr| *addr != SocketAddr::from(requester)));
        assert!(!selected.contains(&SocketAddr::from(Peer::for_test(5003))));
    }
}
