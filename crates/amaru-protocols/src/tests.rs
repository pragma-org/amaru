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

use crate::manager;
use crate::manager::{Manager, ManagerMessage};
use crate::store_effects::ResourceHeaderStore;
use amaru_kernel::peer::Peer;
use amaru_kernel::protocol_messages::network_magic::NetworkMagic;
use amaru_mempool::InMemoryMempool;
use amaru_network::connection::TokioConnections;
use amaru_ouroboros_traits::can_validate_blocks::CanValidateHeaders;
use amaru_ouroboros_traits::can_validate_blocks::mock::{
    MockCanValidateBlocks, MockCanValidateHeaders,
};
use amaru_ouroboros_traits::in_memory_consensus_store::InMemConsensusStore;
use amaru_ouroboros_traits::{
    CanValidateBlocks, ConnectionId, ConnectionProvider, ConnectionResource, ResourceMempool,
};
use pallas_primitives::conway::Tx;
use pure_stage::tokio::{TokioBuilder, TokioRunning};
use pure_stage::{Effects, StageGraph, StageRef};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Handle;
use tokio::time::timeout;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[tokio::test]
#[ignore]
async fn connect_initiator() -> anyhow::Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_test_writer()
        .try_init();

    info!("Creating the initiator");
    let peer = Peer::new(
        SocketAddr::from(([127, 0, 0, 1], 3005))
            .to_string()
            .as_str(),
    );
    let mut initiator_network = TokioBuilder::default();
    let initiator_manager = Manager::new(NetworkMagic::PREPROD, StageRef::blackhole());
    let initiator_stage = initiator_network.stage("initiator", manager::stage);
    let initiator_stage = initiator_network.wire_up(initiator_stage, initiator_manager);
    let initiator_sender = initiator_network.input(initiator_stage);
    let initiator_connections = TokioConnections::new(65535);
    add_resources(&mut initiator_network, initiator_connections);

    info!("Start the initiator");
    let running_initiator = initiator_network.run(Handle::current());
    initiator_sender
        .send(ManagerMessage::AddPeer(peer.clone()))
        .await
        .unwrap();

    wait_for(running_initiator, Duration::from_secs(2000)).await?;
    Ok(())
}

#[tokio::test]
#[ignore]
async fn connect_responder() -> anyhow::Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_test_writer()
        .try_init();

    info!("Creating the responder");
    let mut responder_network = TokioBuilder::default();
    let responder_manager = Manager::new(NetworkMagic::PREPROD, StageRef::blackhole());
    let responder_stage = responder_network.stage("responder", manager::stage);
    let responder_stage = responder_network.wire_up(responder_stage, responder_manager);
    let accept_stage = responder_network.stage("accept", accept_stage);
    let responder_sender = responder_network.input(&responder_stage);
    let accept_stage = responder_network.wire_up(accept_stage, responder_stage.without_state());
    let accept_sender = responder_network.input(accept_stage);
    // Create a connection that notifies the accept stage about new connections
    let responder_connections = TokioConnections::new(65535).with_accept_sender(accept_sender);
    let peer_addr = responder_connections
        .bind(SocketAddr::from(([127, 0, 0, 1], 3005)))
        .await?;
    add_resources(&mut responder_network, responder_connections);
    info!("Responder listening on {}", peer_addr);

    info!("Start the responder");
    let running_responder = responder_network.run(Handle::current());
    responder_sender.send(ManagerMessage::Accept).await.unwrap();

    wait_for(running_responder, Duration::from_secs(2000)).await?;
    Ok(())
}

/// Create a stage that accepts incoming connections and notifies the manager
/// about them. This can not be implemented using contramap because we need to
/// create a sender for that stage and this is not possible with an adapted stage.
pub async fn accept_stage(
    manager_stage: StageRef<ManagerMessage>,
    msg: (Peer, ConnectionId),
    eff: Effects<(Peer, ConnectionId)>,
) -> StageRef<ManagerMessage> {
    let (peer, connection_id) = msg;
    eff.send(
        &manager_stage,
        ManagerMessage::Accepted(peer, connection_id),
    )
    .await;
    manager_stage
}

async fn wait_for(running_responder: TokioRunning, duration: Duration) -> anyhow::Result<()> {
    match timeout(duration, running_responder.join()).await {
        Ok(_) => anyhow::bail!("test should have timed out"),
        Err(_) => {
            info!("test timed out as expected");
        }
    };
    Ok(())
}

pub type ResourceBlockValidation = Arc<dyn CanValidateBlocks + Send + Sync>;
pub type ResourceHeaderValidation = Arc<dyn CanValidateHeaders + Send + Sync>;

fn add_resources(network: &mut TokioBuilder, connections: TokioConnections) {
    let chain_store = Arc::new(InMemConsensusStore::default());
    network
        .resources()
        .put::<ResourceHeaderStore>(chain_store.clone());
    network
        .resources()
        .put::<ResourceBlockValidation>(Arc::new(MockCanValidateBlocks));
    network
        .resources()
        .put::<ResourceHeaderValidation>(Arc::new(MockCanValidateHeaders));
    network
        .resources()
        .put::<ConnectionResource>(Arc::new(connections));
    network
        .resources()
        .put::<ResourceMempool<Tx>>(Arc::new(InMemoryMempool::default()));
}
