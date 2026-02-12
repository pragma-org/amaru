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

use crate::ToSocketAddrs;
use amaru_kernel::{NonEmptyBytes, Peer};
use std::{fmt, net::SocketAddr, num::NonZeroUsize, pin::Pin, sync::Arc, time::Duration};

/// A trait for providing network connections.
/// This is used to abstract over different network implementations, such as TCP,
/// or in-memory connections for testing.
pub trait ConnectionProvider: Send + Sync + 'static {
    fn listen(&self, addr: SocketAddr) -> BoxFuture<'static, std::io::Result<SocketAddr>>;

    fn accept(&self) -> BoxFuture<'static, std::io::Result<(Peer, ConnectionId)>>;

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

pub(crate) type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(
    Debug, PartialEq, Eq, Hash, Clone, Copy, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct ConnectionId(u64);

impl ConnectionId {
    /// Get the next ConnectionId, wrapping on overflow (which should not happen given we are using u64)
    pub fn next(&self) -> Self {
        Self(self.0.wrapping_add(1))
    }

    pub fn initial() -> Self {
        Self(0)
    }
}

impl fmt::Display for ConnectionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

pub type ConnectionsResource = Arc<dyn ConnectionProvider>;
