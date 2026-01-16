// Copyright 2025 PRAGMA
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

use amaru_kernel::bytes::NonEmptyBytes;
use std::{
    fmt,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6},
    num::NonZeroUsize,
    pin::Pin,
    sync::Arc,
    time::Duration,
};

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(
    Debug,
    Default,
    PartialEq,
    Eq,
    Hash,
    Clone,
    Copy,
    PartialOrd,
    Ord,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct ConnectionId(u64);

impl ConnectionId {
    /// Get the next ConnectionId, wrapping on overflow (which should not happen given we are using u64)
    pub fn next(&self) -> Self {
        Self(self.0.wrapping_add(1))
    }
}

impl fmt::Display for ConnectionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ToSocketAddrs {
    SocketAddrs(Vec<SocketAddr>),
    SocketAddrV4(SocketAddrV4),
    SocketAddrV6(SocketAddrV6),
    IpAddrs(Vec<(IpAddr, u16)>),
    IpAddrV4(Ipv4Addr, u16),
    IpAddrV6(Ipv6Addr, u16),
    String(String),
}

impl From<Vec<SocketAddr>> for ToSocketAddrs {
    fn from(addr: Vec<SocketAddr>) -> Self {
        Self::SocketAddrs(addr)
    }
}

impl From<Vec<(IpAddr, u16)>> for ToSocketAddrs {
    fn from(addr: Vec<(IpAddr, u16)>) -> Self {
        Self::IpAddrs(addr)
    }
}

impl From<String> for ToSocketAddrs {
    fn from(addr: String) -> Self {
        Self::String(addr)
    }
}

impl From<(IpAddr, u16)> for ToSocketAddrs {
    fn from((addr, port): (IpAddr, u16)) -> Self {
        match addr {
            IpAddr::V4(ipv4_addr) => Self::IpAddrV4(ipv4_addr, port),
            IpAddr::V6(ipv6_addr) => Self::IpAddrV6(ipv6_addr, port),
        }
    }
}

impl From<(Ipv4Addr, u16)> for ToSocketAddrs {
    fn from((addr, port): (Ipv4Addr, u16)) -> Self {
        Self::IpAddrV4(addr, port)
    }
}

impl From<(Ipv6Addr, u16)> for ToSocketAddrs {
    fn from((addr, port): (Ipv6Addr, u16)) -> Self {
        Self::IpAddrV6(addr, port)
    }
}

impl From<SocketAddr> for ToSocketAddrs {
    fn from(addr: SocketAddr) -> Self {
        match addr {
            SocketAddr::V4(addr) => Self::SocketAddrV4(addr),
            SocketAddr::V6(addr) => Self::SocketAddrV6(addr),
        }
    }
}

impl From<SocketAddrV4> for ToSocketAddrs {
    fn from(addr: SocketAddrV4) -> Self {
        Self::SocketAddrV4(addr)
    }
}

impl From<SocketAddrV6> for ToSocketAddrs {
    fn from(addr: SocketAddrV6) -> Self {
        Self::SocketAddrV6(addr)
    }
}

pub type ConnectionResource = Arc<dyn ConnectionProvider>;

pub trait ConnectionProvider: Send + Sync + 'static {
    fn connect(
        &self,
        addr: Vec<SocketAddr>,
        timeout: Duration,
    ) -> BoxFuture<'static, std::io::Result<ConnectionId>>;

    fn connect_addrs(
        &self,
        addr: ToSocketAddrs,
        timeout: Duration,
    ) -> BoxFuture<'static, std::io::Result<ConnectionId>>;

    fn send(
        &self,
        conn: ConnectionId,
        data: NonEmptyBytes,
    ) -> BoxFuture<'static, std::io::Result<()>>;

    fn recv(
        &self,
        conn: ConnectionId,
        bytes: NonZeroUsize,
    ) -> BoxFuture<'static, std::io::Result<NonEmptyBytes>>;

    fn close(&self, conn: ConnectionId) -> BoxFuture<'static, std::io::Result<()>>;
}
