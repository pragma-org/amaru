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

use anyhow::anyhow;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};

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

impl ToSocketAddrs {
    /// Translate a `ToStockAddrs` back to a vector of `SocketAddr`
    pub fn to_socket_addrs(self) -> anyhow::Result<Vec<SocketAddr>> {
        let addresses = match self {
            ToSocketAddrs::SocketAddrs(addrs) => addrs,
            ToSocketAddrs::SocketAddrV4(a) => vec![SocketAddr::V4(a)],
            ToSocketAddrs::SocketAddrV6(a) => vec![SocketAddr::V6(a)],
            ToSocketAddrs::String(s) => {
                vec![
                    s.parse()
                        .map_err(|e| anyhow!(format!("invalid address '{s}': {e}")))?,
                ]
            }
            ToSocketAddrs::IpAddrs(ips) => ips
                .into_iter()
                .map(|(ip, port)| SocketAddr::new(ip, port))
                .collect(),
            ToSocketAddrs::IpAddrV4(ip, port) => {
                vec![SocketAddr::new(IpAddr::V4(ip), port)]
            }
            ToSocketAddrs::IpAddrV6(ip, port) => {
                vec![SocketAddr::new(IpAddr::V6(ip), port)]
            }
        };
        Ok(addresses)
    }
}
