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

//! Name resolution for [`PeerCandidate`].
//!
//! Yields at most one [`Peer`]. Host lookups take the first viable A/AAAA. SRV lookups try records
//! in RFC 2782 priority order (lower first) and stop at the first viable target address.
//!
//! SRV uses the system DNS resolver, which [`init_resolver`] loads at Tokio node start. A missing
//! or unreadable system config is fatal there; lookup failures remain per-candidate.

use std::{
    fmt,
    net::{IpAddr, SocketAddr},
    sync::OnceLock,
};

use amaru_kernel::{DnsName, Peer, PeerCandidate};
use amaru_observability::warn;
use hickory_resolver::{TokioResolver, proto::rr::rdata::SRV};

static RESOLVER: OnceLock<TokioResolver> = OnceLock::new();

/// System DNS configuration could not be loaded (`/etc/resolv.conf` or the Windows registry).
#[derive(Debug)]
pub struct ResolverInitError {
    message: String,
}

impl fmt::Display for ResolverInitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "failed to load system DNS configuration: {}", self.message)
    }
}

impl std::error::Error for ResolverInitError {}

/// Load the system DNS resolver. Must succeed before any SRV lookup; the Tokio node calls this
/// at start so a missing or unreadable resolver config aborts instead of starving later dials.
pub fn init_resolver() -> Result<(), ResolverInitError> {
    if RESOLVER.get().is_some() {
        return Ok(());
    }
    let builder = TokioResolver::builder_tokio().map_err(|error| ResolverInitError { message: error.to_string() })?;
    let _ = RESOLVER.set(builder.build());
    Ok(())
}

/// Resolve a bootstrap candidate to at most one dialable [`Peer`].
pub async fn resolve_peer_candidate(candidate: &PeerCandidate) -> Option<Peer> {
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

#[expect(clippy::expect_used)]
fn resolver() -> &'static TokioResolver {
    RESOLVER.get().expect("init_resolver must succeed before SRV lookups")
}

async fn resolve_srv(name: &DnsName) -> Option<Peer> {
    let query = PeerCandidate::cardano_srv_name(name);
    let resolver = resolver();
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
    fn init_resolver_loads_system_config() {
        init_resolver().expect("test hosts have a system DNS resolver");
        init_resolver().expect("init_resolver is idempotent");
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
