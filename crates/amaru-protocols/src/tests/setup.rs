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

use crate::store_effects::ResourceHeaderStore;
use amaru_kernel::{BlockHeader, Transaction};
use amaru_network::connection::TokioConnections;
use amaru_ouroboros_traits::can_validate_blocks::CanValidateHeaders;
use amaru_ouroboros_traits::can_validate_blocks::mock::{
    MockCanValidateBlocks, MockCanValidateHeaders,
};
use amaru_ouroboros_traits::{
    CanValidateBlocks, ChainStore, ConnectionsResource, Mempool, ResourceMempool,
};
use pure_stage::StageGraph;
use pure_stage::tokio::TokioBuilder;
use socket2::{Domain, Protocol, Socket, Type};
use std::net::SocketAddr;
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

/// Log to the console (enable logs with the RUST_LOG env var, for example RUST_LOG=info)
pub(super) fn setup_logging(enable: bool) {
    if !enable {
        return;
    };
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_test_writer()
        .try_init();
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

/// Add resources for each role.
/// In particular set up an in-memory chain store with different chain lengths for the initiator and responder.
pub(super) fn set_resources(
    chain_store: Arc<dyn ChainStore<BlockHeader>>,
    mempool: Arc<dyn Mempool<Transaction>>,
    network: &mut TokioBuilder,
    connections: TokioConnections,
) -> anyhow::Result<()> {
    network.resources().put::<ResourceHeaderStore>(chain_store);
    network
        .resources()
        .put::<ResourceBlockValidation>(Arc::new(MockCanValidateBlocks));
    network
        .resources()
        .put::<ResourceHeaderValidation>(Arc::new(MockCanValidateHeaders));
    network
        .resources()
        .put::<ConnectionsResource>(Arc::new(connections));
    network
        .resources()
        .put::<ResourceMempool<Transaction>>(mempool);
    Ok(())
}
