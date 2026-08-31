// Copyright 2026 PRAGMA
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Name resolution for [`PeerCandidate`] as a detached external effect.

use std::collections::BTreeSet;

use amaru_kernel::{Peer, PeerCandidate};
use amaru_observability::warn;
use amaru_pure_stage::{BoxFuture, DurationDist, ExternalEffectAPI, Resources, SendData};

use crate::performance::PeerSource;

/// Result of resolving a bootstrap candidate to zero or more [`Peer`]s.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ResolvePeerCandidateResult {
    pub candidate: PeerCandidate,
    pub origin: PeerSource,
    pub peers: BTreeSet<Peer>,
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
            let peers = resolve_candidate(&this.candidate).await;
            ResolvePeerCandidateResult { candidate: this.candidate, origin: this.origin, peers }
        })
    }
}

async fn resolve_candidate(candidate: &PeerCandidate) -> BTreeSet<Peer> {
    match candidate {
        PeerCandidate::Socket(peer) => BTreeSet::from([*peer]),
        PeerCandidate::Host { host, port } => resolve_host(host.as_str(), *port).await,
        PeerCandidate::Srv { name } => {
            let query = PeerCandidate::cardano_srv_name(name);
            warn!(
                protocols::peer_selection::peer::RESOLVE_FAILED,
                candidate = query.as_str(),
                reason = "SRV lookup is not implemented"
            );
            BTreeSet::new()
        }
    }
}

async fn resolve_host(host: &str, port: u16) -> BTreeSet<Peer> {
    let lookup = format!("{host}:{port}");
    let addrs = match tokio::net::lookup_host(&lookup).await {
        Ok(addrs) => addrs,
        Err(error) => {
            warn!(
                protocols::peer_selection::peer::RESOLVE_FAILED,
                candidate = lookup.as_str(),
                reason = error.to_string()
            );
            return BTreeSet::new();
        }
    };
    let mut peers = BTreeSet::new();
    for addr in addrs {
        match Peer::try_from(addr) {
            Ok(peer) => {
                peers.insert(peer);
            }
            Err(reason) => {
                warn!(
                    protocols::peer_selection::peer::ADDRESS_REJECTED,
                    address = addr.to_string(),
                    reason = reason.to_string()
                );
            }
        }
    }
    peers
}
