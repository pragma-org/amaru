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

use std::{net::SocketAddr, sync::Arc, time::Duration};

use amaru_kernel::{EraHistory, NetworkMagic, NetworkName, Peer};
use amaru_network::connection::TokioConnections;
use amaru_ouroboros_traits::ConnectionProvider;
use pure_stage::{
    StageGraph, StageRef,
    tokio::{TokioBuilder, TokioRunning},
};
use tokio::{runtime::Handle, sync::Notify};

use crate::{
    chainsync::ChainSyncInitiatorMsg,
    manager,
    manager::{Manager, ManagerConfig, ManagerMessage},
    tests::{
        accept_stage::{AcceptState, PullAccept, accept_stage},
        assertions::{check_state, wait_for_termination},
        chainsync_stage::{ChainSyncStageState, test_chainsync_stage},
        configuration::Configuration,
        setup::{
            FailingConnectionProvider, ephemeral_localhost_addr, set_resources, set_resources_with_connections,
            setup_logging,
        },
        slow_manager_stage::slow_manager_stage,
    },
};

/// This test simulates a connection between an initiator and a responder over TCP.
/// We want to observe that eventually the initiator catches up with the responder's chain
/// and that both nodes have the same blocks and transactions.
#[tokio::test]
async fn test_connect_initiator_responder() -> anyhow::Result<()> {
    setup_logging();
    let (responder, addr, responder_done) = start_responder().await?;
    let (initiator, initiator_done) = start_initiator_at(addr).await?;

    wait_for_termination(responder_done, initiator_done).await?;
    check_state(initiator, responder)?;
    Ok(())
}

#[tokio::test]
async fn test_connect_initiator_reconnection() -> anyhow::Result<()> {
    setup_logging();
    let addr = ephemeral_localhost_addr()?;
    tracing::info!("starting test at address {}", addr);
    let (initiator, initiator_done) = start_initiator_with_configuration(
        Configuration::initiator().with_addr(addr).with_reconnect_delay(Duration::from_millis(500)),
    )
    .await?;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let (responder, responder_done) = start_responder_at(addr).await?;
    wait_for_termination(responder_done, initiator_done).await?;
    check_state(initiator, responder)?;

    Ok(())
}

#[tokio::test]
async fn test_connect_initiator_reconnection_on_responder_restart() -> anyhow::Result<()> {
    setup_logging();

    // start an initiator with a responder that will be slow right after connection, so that
    // the initiator doesn't start synchronizing right away
    let responder_configuration = Configuration::responder().with_slow_manager();
    let (responder, addr, _) = start_responder_with_configuration(responder_configuration.clone()).await?;
    let (initiator, initiator_done) = start_initiator_with_configuration(
        Configuration::initiator()
            .with_addr(addr)
            .with_reconnect_delay(Duration::from_millis(500))
            .with_processing_wait(Duration::from_millis(100)),
    )
    .await?;
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Stop the responder
    // Wait a bit to make sure that resources are cleaned up
    tracing::info!("kill the responder at address {}", addr);
    responder.abort();
    tokio::time::sleep(Duration::from_secs(5)).await;

    // Restart the responder normally
    tracing::info!("restart the responder at address {}", addr);
    let responder_configuration = Configuration::responder().with_addr(addr);
    let (responder, _, responder_done) = start_responder_with_configuration(responder_configuration).await?;

    // Both the initiator and the responder should eventually terminate with the same state
    tracing::info!("wait for termination");
    wait_for_termination(responder_done, initiator_done).await?;
    check_state(initiator, responder)?;

    Ok(())
}

/// Test that the accept stage supervision restarts the listener when the accept stage fails.
/// This test uses a FailingConnectionProvider that fails after accepting one connection,
/// which triggers the supervision to restart the listener via ManagerMessage::Listen.
#[tokio::test]
async fn test_accept_stage_supervised_restart() -> anyhow::Result<()> {
    setup_logging();

    // Start a responder with a FailingConnectionProvider that:
    // - Succeeds for the first accept (fail_after=1)
    // - Then fails once (fail_count=1), triggering a supervised restart
    // - Then succeeds for all subsequent accepts
    // The manager will handle ManagerMessage::Listen and create a supervised accept stage.
    // When the accept fails, the supervision will send Listen again, restarting the listener.
    let (responder, addr, responder_done) =
        start_responder_with_failing_accept(Configuration::responder(), 1, 1).await?;

    // Start an initiator that will connect to the responder
    let (initiator, initiator_done) = start_initiator_with_configuration(
        Configuration::initiator()
            .with_addr(addr)
            .with_reconnect_delay(Duration::from_millis(500))
            .with_processing_wait(Duration::from_millis(100)),
    )
    .await?;

    // Both the initiator and the responder should eventually terminate with the same state.
    // This proves that after the accept stage failed and was restarted via supervision,
    // the responder was able to accept new connections and complete the sync.
    tracing::info!("wait for termination");
    wait_for_termination(responder_done, initiator_done).await?;
    check_state(initiator, responder)?;

    Ok(())
}

/// Create and start a responder node that listens for incoming connections on a free port found at
/// runtime. Return the address it is listening on.
async fn start_responder() -> anyhow::Result<(TokioRunning, SocketAddr, Arc<Notify>)> {
    start_responder_with_configuration(Configuration::responder()).await
}

/// Create and start a responder node that listens for incoming connections on an explicit address.
async fn start_responder_at(addr: SocketAddr) -> anyhow::Result<(TokioRunning, Arc<Notify>)> {
    let (responder, _, responder_done) =
        start_responder_with_configuration(Configuration::responder().with_addr(addr)).await?;
    Ok((responder, responder_done))
}

/// Create and start a responder node that listens for incoming connections on a free port found at
/// runtime. Return the address it is listening on.
async fn start_responder_with_configuration(
    configuration: Configuration,
) -> anyhow::Result<(TokioRunning, SocketAddr, Arc<Notify>)> {
    tracing::info!("Creating the responder");
    let mut responder_network = TokioBuilder::default();
    let responder_stage = if configuration.slow_manager {
        responder_network.stage("responder", slow_manager_stage)
    } else {
        responder_network.stage("responder", manager::stage)
    };
    let responder_manager = create_manager(ManagerConfig::default(), StageRef::blackhole());
    let responder_stage = responder_network.wire_up(responder_stage, responder_manager);

    // Create a connection that notifies the accept stage about new connections
    // Note: we need to call listen() first to get the listener address for the accept stage
    let responder_connections = TokioConnections::new(CONNECTION_BUFFER_SIZE);
    let peer_addr = responder_connections.listen(configuration.addr).await?;
    tracing::info!("Responder listening on {}", peer_addr);

    let accept_stage = responder_network.stage("accept", accept_stage);
    let notify = Arc::new(Notify::new());
    let accept_stage = responder_network
        .wire_up(accept_stage, AcceptState::new(responder_stage.without_state(), notify.clone(), peer_addr));
    responder_network.preload(accept_stage, [PullAccept]).unwrap();

    set_resources(configuration.chain_store, configuration.mempool, &mut responder_network, responder_connections)?;

    tracing::info!("Start the responder");
    let running_responder = responder_network.run(Handle::current());
    Ok((running_responder, peer_addr, notify))
}

/// Create and start an initiator node that connects to the given port.
/// ChainSync events are sent to a stage that stores them in the in-memory store without further processing.
async fn start_initiator_at(addr: impl Into<Option<SocketAddr>>) -> anyhow::Result<(TokioRunning, Arc<Notify>)> {
    let addr = addr.into().unwrap_or_else(|| SocketAddr::from(([127, 0, 0, 1], 3000)));
    start_initiator_with_configuration(Configuration::initiator().with_addr(addr)).await
}

/// Create and start an initiator node that connects to the given port.
/// ChainSync events are sent to a stage that stores them in the in-memory store without further processing.
async fn start_initiator_with_configuration(
    configuration: Configuration,
) -> anyhow::Result<(TokioRunning, Arc<Notify>)> {
    tracing::info!("Creating the initiator");
    let addr = configuration.addr;
    let peer = Peer::from_addr(&addr);
    let mut initiator_network = TokioBuilder::default();

    // create stages
    let initiator_stage = initiator_network.stage("initiator", manager::stage);
    let chainsync_stage = initiator_network.stage("chainsync", test_chainsync_stage);

    // wire up stages
    let notify = Arc::new(Notify::new());
    let chainsync_stage = initiator_network.wire_up(
        chainsync_stage,
        ChainSyncStageState::new(initiator_stage.sender(), configuration.processing_wait, notify.clone()),
    );

    let manager_config = ManagerConfig::default().with_reconnect_delay(configuration.reconnect_delay);
    let initiator_manager = create_manager(manager_config, chainsync_stage.without_state());
    let initiator_stage = initiator_network.wire_up(initiator_stage, initiator_manager);
    let initiator_sender = initiator_network.input(initiator_stage);

    let initiator_connections = TokioConnections::new(CONNECTION_BUFFER_SIZE);
    set_resources(configuration.chain_store, configuration.mempool, &mut initiator_network, initiator_connections)?;

    tracing::info!("Start the initiator");
    let running_initiator = initiator_network.run(Handle::current());
    initiator_sender.send(ManagerMessage::AddPeer(peer.clone())).await.unwrap();

    Ok((running_initiator, notify))
}

/// Create and start a responder node that uses the manager's supervised accept stage.
/// This setup tests the supervision restart mechanism: when the accept stage fails,
/// the manager receives a tombstone message (ManagerMessage::Listen) and restarts the listener.
///
/// Parameters:
/// - `fail_after`: Number of successful accepts before failures start
/// - `fail_count`: Number of failures to simulate before returning to normal operation
async fn start_responder_with_failing_accept(
    configuration: Configuration,
    fail_after: usize,
    fail_count: usize,
) -> anyhow::Result<(TokioRunning, SocketAddr, Arc<Notify>)> {
    tracing::info!("Creating the responder with failing accept (fail_after={}, fail_count={})", fail_after, fail_count);
    let mut responder_network = TokioBuilder::default();

    // Create the chainsync stage for tracking completion
    let chainsync_stage = responder_network.stage("chainsync", test_chainsync_stage);
    let notify = Arc::new(Notify::new());

    // Create the manager stage - it will handle ManagerMessage::Listen and create the supervised accept stage
    let responder_stage = responder_network.stage("responder", manager::stage);

    // Wire up the chainsync stage first (manager needs its reference)
    let chainsync_stage = responder_network
        .wire_up(chainsync_stage, ChainSyncStageState::new(responder_stage.sender(), None, notify.clone()));
    let responder_manager = create_manager(ManagerConfig::default(), chainsync_stage.without_state());

    // Wire up the manager stage
    let responder_stage = responder_network.wire_up(responder_stage, responder_manager);
    let responder_sender = responder_network.input(responder_stage);

    // Create a FailingConnectionProvider that wraps TokioConnections.
    // First, we need to listen to get the actual bound address.
    let inner_connections = TokioConnections::new(CONNECTION_BUFFER_SIZE);
    let peer_addr = inner_connections.listen(configuration.addr).await?;
    tracing::info!("Responder will listen on {}", peer_addr);

    // Create the failing provider - it will be used by the manager when it handles Listen
    let failing_connections = FailingConnectionProvider::new(inner_connections, fail_after, fail_count);

    set_resources_with_connections(
        configuration.chain_store,
        configuration.mempool,
        &mut responder_network,
        Arc::new(failing_connections),
    )?;

    tracing::info!("Start the responder");
    let running_responder = responder_network.run(Handle::current());

    // Send ManagerMessage::Listen to trigger the manager to create the supervised accept stage.
    // Note: The listen() call is idempotent, so calling it again on the same address will work.
    responder_sender.send(ManagerMessage::Listen(peer_addr)).await.unwrap();

    Ok((running_responder, peer_addr, notify))
}

/// Buffer size for TCP connections in tests.
const CONNECTION_BUFFER_SIZE: usize = 65535;

fn era_history() -> Arc<EraHistory> {
    let era_history: &EraHistory = NetworkName::Preprod.into();
    Arc::new(era_history.clone())
}

fn create_manager(config: ManagerConfig, chainsync_stage: StageRef<ChainSyncInitiatorMsg>) -> Manager {
    Manager::new(NetworkMagic::PREPROD, config, era_history(), chainsync_stage, StageRef::blackhole())
}
