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

use std::{
    net::SocketAddr,
    num::NonZeroUsize,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use amaru_kernel::{BlockHeader, NonEmptyBytes, Peer, Transaction};
use amaru_network::connection::TokioConnections;
use amaru_ouroboros_traits::{
    CanValidateBlocks, CanValidateHeaders, CanValidateTxs, ChainStore, ConnectionId, ConnectionProvider,
    ConnectionsResource, Mempool, MockCanValidateBlocks, MockCanValidateHeaders, MockCanValidateTxs, ResourceMempool,
    ToSocketAddrs,
};
use pure_stage::{BoxFuture, StageGraph, tokio::TokioBuilder};
use socket2::{Domain, Protocol, Socket, Type};
use tracing_subscriber::EnvFilter;

use crate::store_effects::ResourceHeaderStore;

/// Log to the console (enable logs with the RUST_LOG env var, for example RUST_LOG=info)
pub(super) fn setup_logging() {
    let _ = tracing_subscriber::fmt().with_env_filter(EnvFilter::from_default_env()).with_test_writer().try_init();
}

/// Get an ephemeral localhost address by binding to port 0
/// Be careful, this does not guarantee that the port will always remain free!
pub(super) fn ephemeral_localhost_addr() -> anyhow::Result<SocketAddr> {
    let socket = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))?;
    socket.set_reuse_address(true)?;
    socket.bind(&SocketAddr::from(([127, 0, 0, 1], 0)).into())?;
    let addr = socket.local_addr()?.as_socket().expect("IPv4 socket");
    Ok(addr)
}

/// Resource type definitions
pub(super) type ResourceBlockValidation = Arc<dyn CanValidateBlocks + Send + Sync>;
pub(super) type ResourceHeaderValidation = Arc<dyn CanValidateHeaders + Send + Sync>;
pub(super) type ResourceTxValidation = Arc<dyn CanValidateTxs + Send + Sync>;

/// Add resources for each role.
/// In particular set up an in-memory chain store with different chain lengths for the initiator and responder.
pub(super) fn set_resources(
    chain_store: Arc<dyn ChainStore<BlockHeader>>,
    mempool: Arc<dyn Mempool<Transaction>>,
    network: &mut TokioBuilder,
    connections: TokioConnections,
) -> anyhow::Result<()> {
    set_resources_with_connections(chain_store, mempool, network, Arc::new(connections))
}

/// Add resources for each role with a custom connection provider.
pub(super) fn set_resources_with_connections(
    chain_store: Arc<dyn ChainStore<BlockHeader>>,
    mempool: Arc<dyn Mempool<Transaction>>,
    network: &mut TokioBuilder,
    connections: ConnectionsResource,
) -> anyhow::Result<()> {
    network.resources().put::<ResourceHeaderStore>(chain_store);
    network.resources().put::<ResourceBlockValidation>(Arc::new(MockCanValidateBlocks));
    network.resources().put::<ResourceHeaderValidation>(Arc::new(MockCanValidateHeaders));
    network.resources().put::<ResourceTxValidation>(Arc::new(MockCanValidateTxs));
    network.resources().put::<ConnectionsResource>(connections);
    network.resources().put::<ResourceMempool<Transaction>>(mempool);
    Ok(())
}

/// A connection provider wrapper that fails `accept()` a configured number of times after some successful calls.
/// This is used to test the supervised restart of the accept stage.
///
/// The behavior is:
/// - First `fail_after` accepts succeed
/// - Next `fail_count` accepts fail (triggering restarts)
/// - All subsequent accepts succeed
pub(super) struct FailingConnectionProvider {
    inner: TokioConnections,
    accept_count: AtomicUsize,
    fail_after: usize,
    fail_count: usize,
}

impl FailingConnectionProvider {
    /// Create a new FailingConnectionProvider that wraps the given TokioConnections.
    ///
    /// - `fail_after`: Number of successful accepts before failures start
    /// - `fail_count`: Number of failures to simulate before returning to normal operation
    pub(super) fn new(inner: TokioConnections, fail_after: usize, fail_count: usize) -> Self {
        Self { inner, accept_count: AtomicUsize::new(0), fail_after, fail_count }
    }
}

impl ConnectionProvider for FailingConnectionProvider {
    fn listen(&self, addr: SocketAddr) -> BoxFuture<'static, std::io::Result<SocketAddr>> {
        self.inner.listen(addr)
    }

    fn accept(&self, listener_addr: SocketAddr) -> BoxFuture<'static, std::io::Result<(Peer, ConnectionId)>> {
        let count = self.accept_count.fetch_add(1, Ordering::SeqCst);
        let fail_after = self.fail_after;
        let fail_count = self.fail_count;
        let inner = self.inner.clone();

        Box::pin(async move {
            // Fail if we're in the failure window: [fail_after, fail_after + fail_count)
            if count >= fail_after && count < fail_after + fail_count {
                tracing::info!(count, fail_after, fail_count, "FailingConnectionProvider: simulating accept failure");
                return Err(std::io::Error::other("simulated accept failure for testing"));
            }
            inner.accept(listener_addr).await
        })
    }

    fn connect(&self, addr: Vec<SocketAddr>, timeout: Duration) -> BoxFuture<'static, std::io::Result<ConnectionId>> {
        self.inner.connect(addr, timeout)
    }

    fn connect_addrs(
        &self,
        addr: ToSocketAddrs,
        timeout: Duration,
    ) -> BoxFuture<'static, std::io::Result<ConnectionId>> {
        self.inner.connect_addrs(addr, timeout)
    }

    fn send(&self, conn: ConnectionId, data: NonEmptyBytes) -> BoxFuture<'static, std::io::Result<()>> {
        self.inner.send(conn, data)
    }

    fn recv(&self, conn: ConnectionId, bytes: NonZeroUsize) -> BoxFuture<'static, std::io::Result<NonEmptyBytes>> {
        self.inner.recv(conn, bytes)
    }

    fn close(&self, conn: ConnectionId) -> BoxFuture<'static, std::io::Result<()>> {
        self.inner.close(conn)
    }
}
