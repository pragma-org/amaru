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
use crate::manager::{Manager, ManagerConfig, ManagerMessage};
use crate::tests::accept_stage::{AcceptState, PullAccept, accept_stage};
use crate::tests::assertions::{check_state, wait_for_termination};
use crate::tests::chainsync_stage::{ChainSyncStageState, test_chainsync_stage};
use crate::tests::configuration::Configuration;
use crate::tests::setup::{ephemeral_localhost_addr, set_resources, setup_logging};
use crate::tests::slow_manager_stage::slow_manager_stage;
use amaru_kernel::{NetworkMagic, NetworkName, Peer};
use amaru_network::connection::TokioConnections;
use amaru_ouroboros_traits::ConnectionProvider;
use amaru_slot_arithmetic::EraHistory;
use pure_stage::tokio::{TokioBuilder, TokioRunning};
use pure_stage::{StageGraph, StageRef};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Handle;
use tokio::sync::Notify;

/// This test simulates a connection between an initiator and a responder over TCP.
/// We want to observe that eventually the initiator catches up with the responder's chain
/// and that both nodes have the same blocks and transactions.
#[tokio::test]
async fn test_connect_initiator_responder() -> anyhow::Result<()> {
    setup_logging(true);
    let (responder, addr, responder_done) = start_responder().await?;
    let (initiator, initiator_done) = start_initiator_at(addr).await?;

    wait_for_termination(responder_done, initiator_done).await?;
    check_state(initiator, responder)?;
    Ok(())
}

#[tokio::test]
async fn test_connect_initiator_reconnection() -> anyhow::Result<()> {
    setup_logging(true);
    let addr = ephemeral_localhost_addr()?;
    tracing::info!("starting test at address {}", addr);
    let (initiator, initiator_done) = start_initiator_with_configuration(
        Configuration::initiator()
            .with_addr(addr)
            .with_connection_timeout(Duration::from_millis(500)),
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
    setup_logging(true);

    // start an initiator with a responder that will be slow right after connection, so that
    // the initiator doesn't start synchronizing right away
    let responder_configuration = Configuration::responder().with_slow_manager();
    let (responder, addr, _) =
        start_responder_with_configuration(responder_configuration.clone()).await?;
    let (initiator, initiator_done) = start_initiator_with_configuration(
        Configuration::initiator()
            .with_addr(addr)
            .with_connection_timeout(Duration::from_millis(500))
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
    let (responder, _, responder_done) =
        start_responder_with_configuration(responder_configuration).await?;

    // Both the initiator and the responder should eventually terminate with the same state
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

    let era_history: &EraHistory = NetworkName::Preprod.into();
    let responder_manager = Manager::new(
        NetworkMagic::PREPROD,
        ManagerConfig::default(),
        StageRef::blackhole(),
        Arc::new(era_history.clone()),
    );

    let responder_stage = if configuration.slow_manager {
        responder_network.stage("responder", slow_manager_stage)
    } else {
        responder_network.stage("responder", manager::stage)
    };
    let responder_stage = responder_network.wire_up(responder_stage, responder_manager);

    let accept_stage = responder_network.stage("accept", accept_stage);
    let notify = Arc::new(Notify::new());
    let accept_stage = responder_network.wire_up(
        accept_stage,
        AcceptState::new(responder_stage.without_state(), notify.clone()),
    );
    responder_network
        .preload(accept_stage, [PullAccept])
        .unwrap();

    // Create a connection that notifies the accept stage about new connections
    let responder_connections = TokioConnections::new(65535);
    let peer_addr = responder_connections.listen(configuration.addr).await?;
    set_resources(
        configuration.chain_store,
        configuration.mempool,
        &mut responder_network,
        responder_connections,
    )?;
    tracing::info!("Responder listening on {}", peer_addr);

    tracing::info!("Start the responder");
    let running_responder = responder_network.run(Handle::current());
    Ok((running_responder, peer_addr, notify))
}

/// Create and start an initiator node that connects to the given port.
/// ChainSync events are sent to a stage that stores them in the in-memory store without further processing.
async fn start_initiator_at(
    addr: impl Into<Option<SocketAddr>>,
) -> anyhow::Result<(TokioRunning, Arc<Notify>)> {
    let addr = addr
        .into()
        .unwrap_or_else(|| SocketAddr::from(([127, 0, 0, 1], 3000)));
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
        ChainSyncStageState::new(
            initiator_stage.sender(),
            configuration.processing_wait,
            notify.clone(),
        ),
    );

    let era_history: &EraHistory = NetworkName::Preprod.into();
    let initiator_manager = Manager::new(
        NetworkMagic::PREPROD,
        ManagerConfig::default(),
        chainsync_stage.without_state(),
        Arc::new(era_history.clone()),
    );
    let initiator_stage = initiator_network.wire_up(initiator_stage, initiator_manager);
    let initiator_sender = initiator_network.input(initiator_stage);

    let initiator_connections = TokioConnections::new(65535);
    set_resources(
        configuration.chain_store,
        configuration.mempool,
        &mut initiator_network,
        initiator_connections,
    )?;

    tracing::info!("Start the initiator");
    let running_initiator = initiator_network.run(Handle::current());
    initiator_sender
        .send(ManagerMessage::AddPeer(peer.clone()))
        .await
        .unwrap();

    Ok((running_initiator, notify))
}
