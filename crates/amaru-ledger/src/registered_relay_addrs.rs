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

use std::{
    collections::BTreeSet,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
};

use amaru_kernel::{Bytes, DnsName, Peer, PeerCandidate, Relay};
use amaru_observability::{info_span, warn};

use crate::store::{ReadStore, StoreError};

pub fn collect_from_read_store(db: &impl ReadStore) -> Result<BTreeSet<PeerCandidate>, StoreError> {
    let span = info_span!(ledger::relays::COLLECT).entered();
    let mut set = BTreeSet::new();
    for (_, row) in db.iter_pools()? {
        for relay in &row.current_params.relays {
            push_relay_candidates(relay, &mut set);
        }
    }
    span.record("count", set.len());
    Ok(set)
}

fn push_relay_candidates(relay: &Relay, set: &mut BTreeSet<PeerCandidate>) {
    match relay {
        Relay::SingleHostAddr(port, ipv4, ipv6) => {
            let Some(port) = nullable_to_port(port) else {
                return;
            };
            push_socket(ipv4.as_ref().and_then(nullable_ipv4_to_ip), port, set);
            push_socket(ipv6.as_ref().and_then(nullable_ipv6_to_ip), port, set);
        }
        Relay::SingleHostName(port, dns) => match nullable_to_port(port) {
            Some(port) => {
                if let Ok(host) = DnsName::new(dns.as_ref()) {
                    set.insert(PeerCandidate::host(host, port));
                }
            }
            None => push_srv(dns.as_ref(), set),
        },
        Relay::MultiHostName(dns) => push_srv(dns.as_ref(), set),
    }
}

fn push_srv(name: &str, set: &mut BTreeSet<PeerCandidate>) {
    if let Ok(name) = DnsName::new(name) {
        set.insert(PeerCandidate::srv(name));
    }
}

fn push_socket(ip: Option<IpAddr>, port: u16, set: &mut BTreeSet<PeerCandidate>) {
    let Some(ip) = ip else {
        return;
    };
    if is_excluded_relay_ip(ip) {
        return;
    }
    let addr = SocketAddr::new(ip, port);
    match Peer::try_from(addr) {
        Ok(peer) => {
            set.insert(PeerCandidate::from(peer));
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

fn nullable_to_port(port: &Option<u32>) -> Option<u16> {
    port.and_then(|p| u16::try_from(p).ok())
}

// NOTE: The Haskell node usees the `iproute` package for writing IP addresses from the ledger, first stores the bytes in word32 in network byte order.
// The ledger then uses putWord32le for serializing those words, swapping their byte order in the byte string.
// https://github.com/kazu-yamamoto/iproute/blob/main/Data/IP/Addr.hs#L400
// https://github.com/IntersectMBO/cardano-ledger/blob/master/libs/cardano-ledger-binary/src/Cardano/Ledger/Binary/Encoding/Encoder.hs#L563
fn nullable_ipv4_to_ip(null: &Bytes) -> Option<IpAddr> {
    let mut bytes = <[u8; 4]>::try_from(null.as_slice()).ok()?;
    bytes.reverse();
    Some(IpAddr::V4(Ipv4Addr::from_octets(bytes)))
}

// NOTE: The Haskell node usees the `iproute` package for writing IP addresses from the ledger, first stores the bytes in word32 in network byte order.
// The ledger then uses putWord32le for serializing those words, swapping their byte order in the byte string.
// https://github.com/kazu-yamamoto/iproute/blob/main/Data/IP/Addr.hs#L431
// https://github.com/IntersectMBO/cardano-ledger/blob/master/libs/cardano-ledger-binary/src/Cardano/Ledger/Binary/Encoding/Encoder.hs#L569
fn nullable_ipv6_to_ip(null: &Bytes) -> Option<IpAddr> {
    let mut bytes = <[u8; 16]>::try_from(null.as_slice()).ok()?;
    bytes[0..4].reverse();
    bytes[4..8].reverse();
    bytes[8..12].reverse();
    bytes[12..16].reverse();
    Some(IpAddr::V6(Ipv6Addr::from_octets(bytes)))
}

fn is_excluded_relay_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_unspecified()
                || v4.is_documentation()
        }
        IpAddr::V6(v6) => {
            v6.is_loopback() || v6.is_unicast_link_local() || is_unique_local_v6(&v6) || v6.is_unspecified()
        }
    }
}

fn is_unique_local_v6(v6: &Ipv6Addr) -> bool {
    (v6.segments()[0] & 0xfe00) == 0xfc00
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ipv4_bytes(addr: Ipv4Addr) -> Bytes {
        let mut bytes = addr.octets();
        bytes.reverse();
        Bytes::from(bytes.to_vec())
    }

    #[test]
    fn single_host_addr_public_ipv4_becomes_socket() {
        let mut set = BTreeSet::new();
        push_relay_candidates(
            &Relay::SingleHostAddr(Some(3001), Some(ipv4_bytes(Ipv4Addr::new(8, 8, 8, 8))), None),
            &mut set,
        );
        assert_eq!(set, BTreeSet::from(["8.8.8.8:3001".parse::<PeerCandidate>().unwrap()]));
    }

    #[test]
    fn private_ipv4_is_excluded() {
        let mut set = BTreeSet::new();
        push_relay_candidates(
            &Relay::SingleHostAddr(Some(3001), Some(ipv4_bytes(Ipv4Addr::new(10, 0, 0, 1))), None),
            &mut set,
        );
        assert!(set.is_empty());
    }

    #[test]
    fn single_host_name_with_port_is_host() {
        let mut set = BTreeSet::new();
        push_relay_candidates(&Relay::SingleHostName(Some(3001), "relay.example".parse().unwrap()), &mut set);
        assert_eq!(set, BTreeSet::from([PeerCandidate::host("relay.example".parse().unwrap(), 3001)]));
    }

    #[test]
    fn single_host_name_without_port_is_srv() {
        let mut set = BTreeSet::new();
        push_relay_candidates(&Relay::SingleHostName(None, "pool.example".parse().unwrap()), &mut set);
        assert_eq!(set, BTreeSet::from([PeerCandidate::srv("pool.example".parse().unwrap())]));
    }

    #[test]
    fn multi_host_name_is_srv() {
        let mut set = BTreeSet::new();
        push_relay_candidates(&Relay::MultiHostName("stake.example".parse().unwrap()), &mut set);
        assert_eq!(set, BTreeSet::from([PeerCandidate::srv("stake.example".parse().unwrap())]));
    }

    #[test]
    fn invalid_dns_name_is_skipped() {
        let mut set = BTreeSet::new();
        push_relay_candidates(&Relay::MultiHostName("not a dns".parse().unwrap()), &mut set);
        assert!(set.is_empty());
    }
}
