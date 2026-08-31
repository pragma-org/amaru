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
//! succeeds. A [`PeerCandidate`] is a bootstrap name that may already be a [`Peer`], a
//! hostname+port (A/AAAA), or a DNS name for SRV lookup.

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

/// A DNS name used by [`PeerCandidate::Host`] and [`PeerCandidate::Srv`].
///
/// Labels are LDH plus optional leading `_` (SRV service/proto). No colons, brackets, or
/// IP literals — those belong on [`Peer`] / [`PeerCandidate::Socket`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize)]
#[serde(transparent)]
pub struct DnsName(String);

const DNS_NAME_MAX: usize = 253;

impl DnsName {
    pub fn new(s: impl AsRef<str>) -> Result<Self, DnsNameError> {
        let s = s.as_ref();
        let s = s.strip_suffix('.').filter(|rest| !rest.is_empty()).unwrap_or(s);
        validate_dns_name(s)?;
        Ok(Self(s.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for DnsName {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for DnsName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for DnsName {
    type Err = DnsNameError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

impl<'de> serde::Deserialize<'de> for DnsName {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = <String as serde::Deserialize>::deserialize(deserializer)?;
        Self::new(s).map_err(serde::de::Error::custom)
    }
}

fn validate_dns_name(s: &str) -> Result<(), DnsNameError> {
    if s.is_empty() {
        return Err(DnsNameError::Empty);
    }
    if s.len() > DNS_NAME_MAX {
        return Err(DnsNameError::TooLong);
    }
    if !s.is_ascii() {
        return Err(DnsNameError::Invalid(s.to_string()));
    }
    if s.parse::<IpAddr>().is_ok() {
        return Err(DnsNameError::LiteralIp(s.to_string()));
    }
    for label in s.split('.') {
        validate_dns_label(label, s)?;
    }
    Ok(())
}

fn validate_dns_label(label: &str, name: &str) -> Result<(), DnsNameError> {
    let b = label.as_bytes();
    if b.is_empty() {
        return Err(DnsNameError::EmptyLabel(name.to_string()));
    }
    if b.len() > 63 {
        return Err(DnsNameError::LabelTooLong(name.to_string()));
    }
    let first = b[0];
    let last = b[b.len() - 1];
    if !dns_label_edge(first) || !dns_label_edge(last) || !b.iter().copied().all(dns_label_char) {
        return Err(DnsNameError::Invalid(name.to_string()));
    }
    Ok(())
}

fn dns_label_edge(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

fn dns_label_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'-' || c == b'_'
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DnsNameError {
    #[error("DNS name is empty")]
    Empty,
    #[error("DNS name is longer than 253 octets")]
    TooLong,
    #[error("DNS name '{0}' contains an empty label")]
    EmptyLabel(String),
    #[error("DNS name '{0}' has a label longer than 63 octets")]
    LabelTooLong(String),
    #[error("DNS name '{0}' is a literal IP address")]
    LiteralIp(String),
    #[error("invalid DNS name '{0}'")]
    Invalid(String),
}

/// A bootstrap name that may already be a [`Peer`] or may need DNS.
///
/// The variant selects the resolution path:
/// - [`Socket`](Self::Socket): already a viable address; no lookup.
/// - [`Host`](Self::Host): A/AAAA lookup of `host`, using `port` on every result.
/// - [`Srv`](Self::Srv): DNS SRV lookup of `name` (targets carry their own ports).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind")]
pub enum PeerCandidate {
    /// Literal IPv4/IPv6 + port. No name resolution.
    #[serde(rename = "socket")]
    Socket(Peer),
    /// Hostname (not a literal IP) plus port.
    #[serde(rename = "host")]
    Host { host: DnsName, port: u16 },
    /// DNS name for CIP-0155 SRV lookup (no port in the name).
    ///
    /// Resolution queries `_cardano._tcp.<name>` (see [`Self::cardano_srv_name`]).
    #[serde(rename = "srv")]
    Srv { name: DnsName },
}

impl From<Peer> for PeerCandidate {
    fn from(peer: Peer) -> Self {
        Self::Socket(peer)
    }
}

impl PeerCandidate {
    pub fn socket(peer: Peer) -> Self {
        Self::Socket(peer)
    }

    pub fn host(host: DnsName, port: u16) -> Self {
        Self::Host { host, port }
    }

    pub fn srv(name: DnsName) -> Self {
        Self::Srv { name }
    }

    /// `Some` iff this candidate needs no DNS.
    pub fn as_peer(&self) -> Option<Peer> {
        match self {
            Self::Socket(peer) => Some(*peer),
            Self::Host { .. } | Self::Srv { .. } => None,
        }
    }

    pub fn as_srv(&self) -> Option<&DnsName> {
        match self {
            Self::Srv { name } => Some(name),
            Self::Socket(_) | Self::Host { .. } => None,
        }
    }

    /// Whether this candidate requires name resolution.
    pub fn needs_resolution(&self) -> bool {
        !matches!(self, Self::Socket(_))
    }

    /// DNS name to query for a CIP-0155 Cardano SRV record (`_cardano._tcp.<name>`).
    ///
    /// The ledger / snapshot stores the domain only; the `_cardano._tcp` prefix is added here
    /// (the registry prefix for the Cardano node, which is TCP-only). If `name` is already a
    /// `_cardano._tcp.` query, it is returned unchanged.
    pub fn cardano_srv_name(name: &DnsName) -> String {
        let name = name.as_str();
        if name.starts_with("_cardano._tcp.") { name.to_string() } else { format!("_cardano._tcp.{name}") }
    }
}

impl fmt::Display for PeerCandidate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Socket(peer) => write!(f, "{peer}"),
            Self::Host { host, port } => write!(f, "{host}:{port}"),
            Self::Srv { name } => write!(f, "{name}"),
        }
    }
}

impl FromStr for PeerCandidate {
    type Err = PeerCandidateParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Err(PeerCandidateParseError::Empty);
        }
        if let Ok(addr) = SocketAddr::from_str(s) {
            return Peer::try_from(addr).map(Self::Socket).map_err(PeerCandidateParseError::Peer);
        }
        if s.parse::<IpAddr>().is_ok() {
            return Err(PeerCandidateParseError::LiteralIpMissingPort(s.to_string()));
        }
        if let Some((host, port)) = s.rsplit_once(':')
            && let Ok(port) = port.parse::<u16>()
        {
            return Ok(Self::Host { host: DnsName::new(host)?, port });
        }
        Ok(Self::Srv { name: DnsName::new(s)? })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PeerCandidateParseError {
    #[error("peer candidate is empty")]
    Empty,
    #[error("literal IP '{0}' requires a port")]
    LiteralIpMissingPort(String),
    #[error(transparent)]
    DnsName(#[from] DnsNameError),
    #[error(transparent)]
    Peer(#[from] PeerError),
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
        assert_eq!(ip, PeerCandidate::socket("10.0.0.1:6000".parse().unwrap()));
        assert_eq!(ip.as_peer(), Some("10.0.0.1:6000".parse().unwrap()));
        let host: PeerCandidate = "relay-a.example:3001".parse().unwrap();
        assert_eq!(host, PeerCandidate::host("relay-a.example".parse().unwrap(), 3001));
        assert_eq!(host.as_peer(), None);
        let srv: PeerCandidate = "example.com".parse().unwrap();
        assert_eq!(srv, PeerCandidate::srv("example.com".parse().unwrap()));
        assert_eq!(srv.as_srv().map(PeerCandidate::cardano_srv_name).as_deref(), Some("_cardano._tcp.example.com"));
        let prefixed: PeerCandidate = "_cardano._tcp.example.com".parse().unwrap();
        assert_eq!(
            prefixed.as_srv().map(PeerCandidate::cardano_srv_name).as_deref(),
            Some("_cardano._tcp.example.com")
        );
        assert!(srv.needs_resolution());
        let err = PeerCandidate::from_str("10.0.0.1").unwrap_err();
        assert!(matches!(err, PeerCandidateParseError::LiteralIpMissingPort(_)));
    }

    #[test]
    fn candidate_rejects_mapped_v4() {
        let err = PeerCandidate::from_str("[::ffff:127.0.0.1]:3001").unwrap_err();
        assert!(matches!(err, PeerCandidateParseError::Peer(PeerError::MappedIpv4(_))));
    }

    #[test]
    fn dns_name_rejects_colons_brackets_and_ips() {
        assert!(DnsName::new("relay.example").is_ok());
        assert!(DnsName::new("_cardano._tcp.example.com").is_ok());
        assert!(DnsName::new("[::1]").is_err());
        assert!(DnsName::new("example.com:3001").is_err());
        assert!(DnsName::new("10.0.0.1").is_err());
        assert!(DnsName::new("").is_err());
        assert!(DnsName::new("bad..name").is_err());
        assert!(matches!(PeerCandidate::from_str("[relay.example]:3001"), Err(PeerCandidateParseError::DnsName(_))));
    }
}
