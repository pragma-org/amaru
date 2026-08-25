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

//! A [`Peer`] is a viable network address, not a node identity and not a promise that dialing
//! succeeds. An unresolved name (including an IP:port still in string form) is a
//! [`PeerCandidate`].

use std::{
    fmt,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6},
    num::NonZeroU16,
    str::FromStr,
};

/// A resolved TCP endpoint used as a peer key (performance, selection, manager).
///
/// `Copy` is a permanent invariant; future transports must intern non-`Copy` identities.
#[derive(
    Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(tag = "peer")]
pub enum Peer {
    #[serde(rename = "ipv4")]
    Ipv4 {
        #[serde(with = "ipv4_str")]
        #[schemars(with = "String")]
        address: Ipv4Addr,
        port: u16,
    },
    #[serde(rename = "ipv6")]
    Ipv6 {
        #[serde(with = "ipv6_str")]
        #[schemars(with = "String")]
        address: Ipv6Addr,
        port: u16,
        flowinfo: u32,
        scope_id: u32,
    },
}

mod ipv4_str {
    use serde::{Deserialize, Deserializer, Serializer};

    use super::*;

    pub fn serialize<S: Serializer>(addr: &Ipv4Addr, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&addr.to_string())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Ipv4Addr, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

mod ipv6_str {
    use serde::{Deserialize, Deserializer, Serializer};

    use super::*;

    pub fn serialize<S: Serializer>(addr: &Ipv6Addr, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&addr.to_string())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Ipv6Addr, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// Why a [`SocketAddr`] or string cannot become a [`Peer`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PeerError {
    #[error("IPv4-mapped IPv6 addresses are not supported: {0}")]
    MappedIpv4(Ipv6Addr),
    #[error("invalid peer address '{input}': {source}")]
    Parse {
        input: String,
        #[source]
        source: std::net::AddrParseError,
    },
}

impl Peer {
    /// Loopback IPv4 peer for tests. Not a production constructor: real peers come from
    /// [`TryFrom<SocketAddr>`] / [`FromStr`].
    #[cfg(any(test, feature = "test-utils"))]
    pub const fn for_test(port: u16) -> Self {
        Self::Ipv4 { address: Ipv4Addr::LOCALHOST, port }
    }

    pub const fn ipv4(address: Ipv4Addr, port: u16) -> Self {
        Self::Ipv4 { address, port }
    }

    pub const fn ipv6(address: Ipv6Addr, port: u16, flowinfo: u32, scope_id: u32) -> Self {
        Self::Ipv6 { address, port, flowinfo, scope_id }
    }

    pub fn port(self) -> u16 {
        match self {
            Self::Ipv4 { port, .. } | Self::Ipv6 { port, .. } => port,
        }
    }

    pub fn ip(self) -> IpAddr {
        match self {
            Self::Ipv4 { address, .. } => IpAddr::V4(address),
            Self::Ipv6 { address, .. } => IpAddr::V6(address),
        }
    }
}

impl fmt::Display for Peer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        SocketAddr::from(*self).fmt(f)
    }
}

impl From<Peer> for SocketAddr {
    fn from(peer: Peer) -> Self {
        match peer {
            Peer::Ipv4 { address, port } => SocketAddr::V4(SocketAddrV4::new(address, port)),
            Peer::Ipv6 { address, port, flowinfo, scope_id } => {
                SocketAddr::V6(SocketAddrV6::new(address, port, flowinfo, scope_id))
            }
        }
    }
}

impl TryFrom<Peer> for SocketAddrV4 {
    type Error = Peer;

    fn try_from(peer: Peer) -> Result<Self, Self::Error> {
        match peer {
            Peer::Ipv4 { address, port } => Ok(SocketAddrV4::new(address, port)),
            ipv6 @ Peer::Ipv6 { .. } => Err(ipv6),
        }
    }
}

impl TryFrom<SocketAddr> for Peer {
    type Error = PeerError;

    fn try_from(addr: SocketAddr) -> Result<Self, Self::Error> {
        match addr {
            SocketAddr::V4(v4) => Ok(Self::from(v4)),
            SocketAddr::V6(v6) => Self::try_from(v6),
        }
    }
}

impl TryFrom<&SocketAddr> for Peer {
    type Error = PeerError;

    fn try_from(addr: &SocketAddr) -> Result<Self, Self::Error> {
        Self::try_from(*addr)
    }
}

impl From<SocketAddrV4> for Peer {
    fn from(addr: SocketAddrV4) -> Self {
        Self::Ipv4 { address: *addr.ip(), port: addr.port() }
    }
}

impl TryFrom<SocketAddrV6> for Peer {
    type Error = PeerError;

    fn try_from(addr: SocketAddrV6) -> Result<Self, Self::Error> {
        let address = *addr.ip();
        if address.to_ipv4_mapped().is_some() {
            return Err(PeerError::MappedIpv4(address));
        }
        // Store flowinfo as zero until we opt into using it; the field still participates in Eq/Ord.
        Ok(Self::Ipv6 { address, port: addr.port(), flowinfo: 0, scope_id: addr.scope_id() })
    }
}

impl From<(Ipv4Addr, u16)> for Peer {
    fn from((address, port): (Ipv4Addr, u16)) -> Self {
        Self::Ipv4 { address, port }
    }
}

impl From<(Ipv4Addr, NonZeroU16)> for Peer {
    fn from((address, port): (Ipv4Addr, NonZeroU16)) -> Self {
        Self::Ipv4 { address, port: port.get() }
    }
}

impl FromStr for Peer {
    type Err = PeerError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let addr = SocketAddr::from_str(s).map_err(|source| PeerError::Parse { input: s.to_string(), source })?;
        Self::try_from(addr)
    }
}

/// An unresolved peer name: hostname, SRV, or an address still in string form.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize)]
pub enum PeerCandidate {
    /// Host or literal IP, with port (`relay.example:3001`, `10.0.0.1:6000`, `[::1]:3001`).
    Host { host: String, port: u16 },
    /// DNS SRV name (resolution may be implemented later).
    Srv { name: String },
}

impl From<Peer> for PeerCandidate {
    fn from(peer: Peer) -> Self {
        match peer {
            Peer::Ipv4 { address, port } => Self::Host { host: address.to_string(), port },
            Peer::Ipv6 { address, port, .. } => Self::Host { host: address.to_string(), port },
        }
    }
}

impl PeerCandidate {
    pub fn host(host: impl Into<String>, port: u16) -> Self {
        Self::Host { host: host.into(), port }
    }

    pub fn srv(name: impl Into<String>) -> Self {
        Self::Srv { name: name.into() }
    }

    /// If this candidate is already a literal IP:port, parse it as a [`Peer`] without DNS.
    pub fn as_literal_peer(&self) -> Option<Peer> {
        match self {
            Self::Host { host, port } => {
                let ip: IpAddr = host.parse().ok()?;
                match ip {
                    IpAddr::V4(address) => Some(Peer::Ipv4 { address, port: *port }),
                    IpAddr::V6(address) => Peer::try_from(SocketAddrV6::new(address, *port, 0, 0)).ok(),
                }
            }
            Self::Srv { .. } => None,
        }
    }
}

impl fmt::Display for PeerCandidate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Host { host, port } => {
                if host.contains(':') && !host.starts_with('[') {
                    write!(f, "[{host}]:{port}")
                } else {
                    write!(f, "{host}:{port}")
                }
            }
            Self::Srv { name } => write!(f, "{name}"),
        }
    }
}

impl FromStr for PeerCandidate {
    type Err = PeerCandidateParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some(host) = s.strip_prefix('[').and_then(|rest| rest.split_once("]:")) {
            let (host, port) = host;
            let port = port.parse().map_err(|_| PeerCandidateParseError::InvalidPort(s.to_string()))?;
            return Ok(Self::Host { host: host.to_string(), port });
        }
        let Some((host, port)) = s.rsplit_once(':') else {
            return Err(PeerCandidateParseError::MissingPort(s.to_string()));
        };
        if host.is_empty() {
            return Err(PeerCandidateParseError::MissingHost(s.to_string()));
        }
        let port = port.parse().map_err(|_| PeerCandidateParseError::InvalidPort(s.to_string()))?;
        Ok(Self::Host { host: host.to_string(), port })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PeerCandidateParseError {
    #[error("peer candidate '{0}' has no port")]
    MissingPort(String),
    #[error("peer candidate '{0}' has no host")]
    MissingHost(String),
    #[error("peer candidate '{0}' has an invalid port")]
    InvalidPort(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_ipv4_is_internally_tagged() {
        let peer = Peer::from((Ipv4Addr::LOCALHOST, 3000));
        let json = serde_json::to_value(peer).unwrap();
        assert_eq!(json, serde_json::json!({"peer":"ipv4","address":"127.0.0.1","port":3000}));
        assert_eq!(serde_json::from_value::<Peer>(json).unwrap(), peer);
    }

    #[test]
    fn rejects_ipv4_mapped_v6() {
        let mapped = SocketAddr::from_str("[::ffff:127.0.0.1]:3001").unwrap();
        assert!(matches!(Peer::try_from(mapped), Err(PeerError::MappedIpv4(_))));
    }

    #[test]
    fn zeros_flowinfo_on_v6_ingest() {
        let addr = SocketAddrV6::new(Ipv6Addr::LOCALHOST, 3001, 99, 1);
        let peer = Peer::try_from(addr).unwrap();
        match peer {
            Peer::Ipv6 { flowinfo, scope_id, .. } => {
                assert_eq!(flowinfo, 0);
                assert_eq!(scope_id, 1);
            }
            Peer::Ipv4 { .. } => panic!("expected ipv6"),
        }
        let other = Peer::ipv6(Ipv6Addr::LOCALHOST, 3001, 0, 2);
        assert_ne!(peer, other);
    }

    #[test]
    fn from_str_literal_address() {
        let peer: Peer = "10.0.0.1:6000".parse().unwrap();
        assert_eq!(peer, Peer::from((Ipv4Addr::new(10, 0, 0, 1), 6000)));
    }

    #[test]
    fn candidate_literal_and_hostname() {
        let ip: PeerCandidate = "10.0.0.1:6000".parse().unwrap();
        assert_eq!(ip.as_literal_peer(), Some("10.0.0.1:6000".parse().unwrap()));
        let host: PeerCandidate = "relay-a.example:3001".parse().unwrap();
        assert_eq!(host, PeerCandidate::host("relay-a.example", 3001));
        assert_eq!(host.as_literal_peer(), None);
    }
}
