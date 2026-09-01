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

//! Name resolution for [`PeerCandidate`] as a detached external effect.
//!
//! Resolution runs **after** a candidate is selected and **before** dialling, and yields at most
//! one [`Peer`]. The lookup itself lives in `amaru-network` (not compiled for wasm/riscv).

use amaru_kernel::{Peer, PeerCandidate};
use amaru_pure_stage::{BoxFuture, DurationDist, ExternalEffectAPI, Resources, SendData};

use crate::performance::PeerSource;

/// Result of resolving a selected candidate to at most one [`Peer`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ResolvePeerCandidateResult {
    pub candidate: PeerCandidate,
    pub origin: PeerSource,
    pub peer: Option<Peer>,
}

/// Resolve a [`PeerCandidate`] that is not already a [`PeerCandidate::Socket`].
///
/// The airlock is acked immediately via [`amaru_pure_stage::Effects::detach`]; the mapped
/// [`ResolvePeerCandidateResult`] is later enqueued on the calling stage.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ResolvePeerCandidate {
    pub candidate: PeerCandidate,
    pub origin: PeerSource,
}

impl ResolvePeerCandidate {
    pub fn new(candidate: PeerCandidate, origin: PeerSource) -> Self {
        Self { candidate, origin }
    }
}

impl ExternalEffectAPI for ResolvePeerCandidate {
    type Response = ResolvePeerCandidateResult;
    const SIMULATED_DURATION: DurationDist = DurationDist::UntilResolved;

    fn run(self: Box<Self>, _resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap(|this| async move {
            let peer = resolve_candidate(&this.candidate).await;
            ResolvePeerCandidateResult { candidate: this.candidate, origin: this.origin, peer }
        })
    }
}

async fn resolve_candidate(candidate: &PeerCandidate) -> Option<Peer> {
    #[cfg(all(not(target_family = "wasm"), not(target_arch = "riscv32")))]
    {
        amaru_network::resolve::resolve_peer_candidate(candidate).await
    }
    #[cfg(any(target_family = "wasm", target_arch = "riscv32"))]
    {
        let _ = candidate;
        None
    }
}
