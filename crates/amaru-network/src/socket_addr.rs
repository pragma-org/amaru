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

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use tokio::net::lookup_host;

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ToSocketAddrs {
    SocketAddr(SocketAddr),
    SocketAddrV4(SocketAddrV4),
    SocketAddrV6(SocketAddrV6),
    IpAddr(IpAddr, u16),
    IpAddrV4(Ipv4Addr, u16),
    IpAddrV6(Ipv6Addr, u16),
    String(String),
}

impl ToSocketAddrs {
    pub async fn resolve(&self) -> std::io::Result<Vec<SocketAddr>> {
        match self {
            Self::SocketAddr(addr) => Ok(vec![*addr]),
            Self::SocketAddrV4(addr) => Ok(vec![(*addr).into()]),
            Self::SocketAddrV6(addr) => Ok(vec![(*addr).into()]),
            Self::IpAddr(addr, port) => Ok(vec![(*addr, *port).into()]),
            Self::IpAddrV4(addr, port) => Ok(vec![(*addr, *port).into()]),
            Self::IpAddrV6(addr, port) => Ok(vec![(*addr, *port).into()]),
            Self::String(addr) => Ok(lookup_host(&addr).await?.take(100).collect()),
        }
    }
}
