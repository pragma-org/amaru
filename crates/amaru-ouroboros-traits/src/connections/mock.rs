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

use crate::connections::BoxFuture;
use crate::{ConnectionId, ConnectionProvider, ToSocketAddrs};
use amaru_kernel::{NonEmptyBytes, Peer};
use std::net::SocketAddr;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct MockConnectionProvider {
    connection_id: Arc<parking_lot::Mutex<ConnectionId>>,
}

impl Default for MockConnectionProvider {
    fn default() -> Self {
        MockConnectionProvider::new(ConnectionId::initial())
    }
}

impl MockConnectionProvider {
    /// Create a new mock connection provider with the given connection ID.
    pub fn new(connection_id: ConnectionId) -> Self {
        Self {
            connection_id: Arc::new(parking_lot::Mutex::new(connection_id)),
        }
    }
}

#[expect(clippy::todo)]
impl ConnectionProvider for MockConnectionProvider {
    fn listen(&self, _addr: SocketAddr) -> BoxFuture<'static, std::io::Result<SocketAddr>> {
        todo!()
    }

    fn accept(&self) -> BoxFuture<'static, std::io::Result<(Peer, ConnectionId)>> {
        todo!()
    }

    fn connect(
        &self,
        _addr: Vec<SocketAddr>,
        _timeout: Duration,
    ) -> BoxFuture<'static, std::io::Result<ConnectionId>> {
        todo!()
    }

    fn connect_addrs(
        &self,
        _addr: ToSocketAddrs,
        _timeout: Duration,
    ) -> BoxFuture<'static, std::io::Result<ConnectionId>> {
        let connection_id = {
            let mut conn_id = self.connection_id.lock();
            *conn_id = conn_id.next();
            *conn_id
        };
        Box::pin(async move { Ok(connection_id) })
    }

    fn send(
        &self,
        _conn: ConnectionId,
        _data: NonEmptyBytes,
    ) -> BoxFuture<'static, std::io::Result<()>> {
        todo!()
    }

    fn recv(
        &self,
        _conn: ConnectionId,
        _bytes: NonZeroUsize,
    ) -> BoxFuture<'static, std::io::Result<NonEmptyBytes>> {
        todo!()
    }

    fn close(&self, _conn: ConnectionId) -> BoxFuture<'static, std::io::Result<()>> {
        todo!()
    }
}
