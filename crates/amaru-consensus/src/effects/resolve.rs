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
//! one [`Peer`]. Host lookups take the first viable A/AAAA. SRV lookups try records in RFC 2782
//! priority order (lower first) and stop at the first viable target address.

use std::{
    net::{IpAddr, SocketAddr},
    sync::OnceLock,
};

use amaru_kernel::{DnsName, Peer, PeerCandidate};
use amaru_observability::warn;
use amaru_pure_stage::{BoxFuture, DurationDist, ExternalEffectAPI, Resources, SendData};
use hickory_resolver::{TokioResolver, proto::rr::rdata::SRV};

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
    match candidate {
        PeerCandidate::Socket(peer) => Some(*peer),
        PeerCandidate::Host { host, port } => resolve_host(host.as_str(), *port).await,
        PeerCandidate::Srv { name } => resolve_srv(name).await,
    }
}

/// RFC 2782 SRV fields we care about for first-viable resolution (no weighted sampling).
#[derive(Clone, Debug, PartialEq, Eq)]
struct SrvChoice {
    priority: u16,
    port: u16,
    target: String,
}

impl SrvChoice {
    fn is_usable(&self) -> bool {
        self.port != 0 && self.target != "." && !self.target.is_empty()
    }
}

fn srv_choice(srv: &SRV) -> Option<SrvChoice> {
    if srv.port() == 0 || srv.target().is_root() {
        return None;
    }
    let target = srv.target().to_string();
    let target = target.trim_end_matches('.').to_string();
    let choice = SrvChoice { priority: srv.priority(), port: srv.port(), target };
    choice.is_usable().then_some(choice)
}

fn ordered_usable_srv(records: impl IntoIterator<Item = SrvChoice>) -> Vec<SrvChoice> {
    let mut records: Vec<_> = records.into_iter().filter(SrvChoice::is_usable).collect();
    records.sort_by_key(|r| r.priority);
    records
}

fn resolver() -> Option<&'static TokioResolver> {
    static RESOLVER: OnceLock<Option<TokioResolver>> = OnceLock::new();
    RESOLVER
        .get_or_init(|| match TokioResolver::builder_tokio() {
            Ok(builder) => Some(builder.build()),
            Err(_) => None,
        })
        .as_ref()
}

async fn resolve_srv(name: &DnsName) -> Option<Peer> {
    let query = PeerCandidate::cardano_srv_name(name);
    let Some(resolver) = resolver() else {
        warn!(
            protocols::peer_selection::peer::RESOLVE_FAILED,
            candidate = query.as_str(),
            reason = "DNS resolver is unavailable"
        );
        return None;
    };
    let lookup = match resolver.srv_lookup(query.as_str()).await {
        Ok(lookup) => lookup,
        Err(error) => {
            warn!(
                protocols::peer_selection::peer::RESOLVE_FAILED,
                candidate = query.as_str(),
                reason = error.to_string()
            );
            return None;
        }
    };
    let records = ordered_usable_srv(lookup.iter().filter_map(srv_choice));
    for record in records {
        if let Some(peer) = resolve_host(&record.target, record.port).await {
            return Some(peer);
        }
    }
    warn!(
        protocols::peer_selection::peer::RESOLVE_FAILED,
        candidate = query.as_str(),
        reason = "no viable SRV address"
    );
    None
}

async fn resolve_host(host: &str, port: u16) -> Option<Peer> {
    if let Ok(ip) = host.parse::<IpAddr>() {
        return first_viable_peer(std::iter::once(SocketAddr::new(ip, port)));
    }
    let lookup = format!("{host}:{port}");
    let addrs = match tokio::net::lookup_host(&lookup).await {
        Ok(addrs) => addrs,
        Err(error) => {
            warn!(
                protocols::peer_selection::peer::RESOLVE_FAILED,
                candidate = lookup.as_str(),
                reason = error.to_string()
            );
            return None;
        }
    };
    let peer = first_viable_peer(addrs);
    if peer.is_none() {
        warn!(
            protocols::peer_selection::peer::RESOLVE_FAILED,
            candidate = lookup.as_str(),
            reason = "no viable address"
        );
    }
    peer
}

fn first_viable_peer(addrs: impl IntoIterator<Item = SocketAddr>) -> Option<Peer> {
    for addr in addrs {
        match Peer::try_from(addr) {
            Ok(peer) => return Some(peer),
            Err(reason) => {
                warn!(
                    protocols::peer_selection::peer::ADDRESS_REJECTED,
                    address = addr.to_string(),
                    reason = reason.to_string()
                );
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use std::{
        net::{Ipv4Addr, Ipv6Addr, SocketAddr},
        str::FromStr,
    };

    use super::*;

    fn v4(octet: u8, port: u16) -> SocketAddr {
        SocketAddr::from((Ipv4Addr::new(10, 0, 0, octet), port))
    }

    #[test]
    fn first_viable_peer_skips_rejected_then_takes_first_ok() {
        let mapped = SocketAddr::from((Ipv6Addr::from_str("::ffff:127.0.0.1").unwrap(), 3001));
        let ok = v4(1, 3001);
        let later = v4(2, 3001);
        assert_eq!(first_viable_peer([mapped, ok, later]), Some(Peer::try_from(ok).unwrap()));
    }

    #[test]
    fn first_viable_peer_none_when_all_rejected() {
        let mapped = SocketAddr::from((Ipv6Addr::from_str("::ffff:10.0.0.1").unwrap(), 3001));
        assert_eq!(first_viable_peer([mapped]), None);
    }

    #[test]
    fn srv_records_are_tried_in_priority_order_skipping_unavailable() {
        let records = ordered_usable_srv([
            SrvChoice { priority: 10, port: 3001, target: "b.example".into() },
            SrvChoice { priority: 0, port: 3001, target: "a.example".into() },
            SrvChoice { priority: 0, port: 0, target: "skip-port.example".into() },
            SrvChoice { priority: 5, port: 3001, target: ".".into() },
            SrvChoice { priority: 1, port: 3001, target: String::new() },
        ]);
        assert_eq!(records.iter().map(|r| r.target.as_str()).collect::<Vec<_>>(), vec!["a.example", "b.example"]);
    }
}
