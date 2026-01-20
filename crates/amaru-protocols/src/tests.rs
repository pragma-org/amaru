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

use crate::chainsync::ChainSyncInitiatorMsg;
use crate::manager::{Manager, ManagerMessage};
use crate::protocol::Role;
use crate::store_effects::{ResourceHeaderStore, Store};
use crate::tx_submission::{create_transactions, create_transactions_in_mempool};
use crate::{chainsync, manager};
use amaru_kernel::is_header::tests::{any_headers_chain_with_root, make_header, run};
use amaru_kernel::peer::Peer;
use amaru_kernel::protocol_messages::network_magic::NetworkMagic;
use amaru_kernel::{BlockHeader, HeaderHash, IsHeader, cbor};
use amaru_mempool::InMemoryMempool;
use amaru_network::connection::TokioConnections;
use amaru_ouroboros_traits::can_validate_blocks::CanValidateHeaders;
use amaru_ouroboros_traits::can_validate_blocks::mock::{
    MockCanValidateBlocks, MockCanValidateHeaders,
};
use amaru_ouroboros_traits::in_memory_consensus_store::InMemConsensusStore;
use amaru_ouroboros_traits::{
    CanValidateBlocks, ChainStore, ConnectionId, ConnectionProvider, ConnectionResource,
    ResourceMempool, TxId,
};
use pallas_primitives::babbage::{Header, MintedHeader};
use pallas_primitives::conway::Tx;
use pure_stage::tokio::{TokioBuilder, TokioRunning};
use pure_stage::{Effects, StageGraph, StageRef};
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Handle;
use tokio::time::timeout;
use tracing_subscriber::EnvFilter;

#[tokio::test]
async fn test_connect_initiator_responder() -> anyhow::Result<()> {
    setup_logging(true);
    let (responder, addr) = start_responder().await?;

    let responder_chain_store = responder.resources().get::<ResourceHeaderStore>()?.clone();
    let responder_mempool = responder.resources().get::<ResourceMempool<Tx>>()?.clone();

    let initiator = start_initiator(addr).await?;
    let initiator_chain_store = initiator.resources().get::<ResourceHeaderStore>()?.clone();
    let initiator_mempool = initiator.resources().get::<ResourceMempool<Tx>>()?.clone();

    tokio::select! {
        res = responder.join() => anyhow::bail!("responder terminated unexpectedly: {res:?}"),
        res = initiator.join() => anyhow::bail!("initiator terminated unexpectedly: {res:?}"),
        _ = tokio::time::sleep(Duration::from_secs(5)) => {
        }
    }

    // Verify that the 2 nodes have the same best chain
    assert_eq!(
        initiator_chain_store.get_best_chain_hash(),
        responder_chain_store.get_best_chain_hash()
    );

    // Verify that the 2 nodes have the same transactions
    let tx_ids = get_tx_ids();
    assert_eq!(
        responder_mempool.get_txs_for_ids(tx_ids.as_slice()),
        initiator_mempool.get_txs_for_ids(tx_ids.as_slice())
    );

    Ok(())
}

/// Create and start a responder node that listens for incoming connections on a free port found at
/// runtime. Return the address it is listening on.
async fn start_responder() -> anyhow::Result<(TokioRunning, SocketAddr)> {
    tracing::info!("Creating the responder");
    let mut responder_network = TokioBuilder::default();
    let responder_manager = Manager::new(NetworkMagic::PREPROD, StageRef::blackhole());

    let responder_stage = responder_network.stage("responder", manager::stage);
    let responder_stage = responder_network.wire_up(responder_stage, responder_manager);
    let responder_sender = responder_network.input(&responder_stage);

    let accept_stage = responder_network.stage("accept", accept_stage);
    let accept_stage = responder_network.wire_up(accept_stage, responder_stage.without_state());
    let accept_sender = responder_network.input(accept_stage);

    // Create a connection that notifies the accept stage about new connections
    let responder_connections = TokioConnections::new(65535).with_accept_sender(accept_sender);
    let peer_addr = responder_connections
        .listen(SocketAddr::from(([127, 0, 0, 1], 0)))
        .await?;
    add_resources(
        &mut responder_network,
        Role::Responder,
        responder_connections,
    )?;
    tracing::info!("Responder listening on {}", peer_addr);

    tracing::info!("Start the responder");
    let running_responder = responder_network.run(Handle::current());
    responder_sender.send(ManagerMessage::Accept).await.unwrap();
    Ok((running_responder, peer_addr))
}

/// Create and start an initiator node that connects to the given port.
/// ChainSync events are sent to a stage that stores them in the in-memory store without further processing.
async fn start_initiator(addr: impl Into<Option<SocketAddr>>) -> anyhow::Result<TokioRunning> {
    tracing::info!("Creating the initiator");
    let addr = addr
        .into()
        .unwrap_or_else(|| SocketAddr::from(([127, 0, 0, 1], 3000)));
    let peer = Peer::from_addr(&addr);
    let mut initiator_network = TokioBuilder::default();

    let chainsync_stage = initiator_network.stage("chainsync", test_chainsync_stage);
    let chainsync_stage = initiator_network.wire_up(chainsync_stage, ());

    let initiator_manager = Manager::new(NetworkMagic::PREPROD, chainsync_stage.without_state());
    let initiator_stage = initiator_network.stage("initiator", manager::stage);
    let initiator_stage = initiator_network.wire_up(initiator_stage, initiator_manager);
    let initiator_sender = initiator_network.input(initiator_stage);

    let initiator_connections = TokioConnections::new(65535);
    add_resources(
        &mut initiator_network,
        Role::Initiator,
        initiator_connections,
    )?;

    tracing::info!("Start the initiator");
    let running_initiator = initiator_network.run(Handle::current());
    initiator_sender
        .send(ManagerMessage::AddPeer(peer.clone()))
        .await
        .unwrap();

    Ok(running_initiator)
}

/// If needed the initiator can be started alone for debugging purposes
#[tokio::test]
#[ignore]
async fn connect_initiator() -> anyhow::Result<()> {
    setup_logging(true);
    let running = start_initiator(None).await?;
    wait_for(running.join(), Duration::from_secs(2000)).await?;
    Ok(())
}

/// If needed the responder can be started alone for debugging purposes
#[tokio::test]
#[ignore]
async fn connect_responder() -> anyhow::Result<()> {
    setup_logging(true);
    let running = start_responder().await?.0;
    wait_for(running.join(), Duration::from_secs(2000)).await?;
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

// HELPERS

/// Log to the console (enable logs with the RUST_LOG env var, for example RUST_LOG=info)
fn setup_logging(enable: bool) {
    if !enable {
        return;
    };
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_test_writer()
        .try_init();
}

/// This is a ChainSync stage that just logs the events
async fn test_chainsync_stage(
    _: (),
    msg: ChainSyncInitiatorMsg,
    eff: Effects<ChainSyncInitiatorMsg>,
) {
    use crate::chainsync::InitiatorResult::*;
    match msg.msg {
        Initialize => {
            tracing::info!(peer = %msg.peer,"initializing chainsync");
        }
        IntersectFound(point, tip) => {
            tracing::info!(peer = %msg.peer, ?point, ?tip, "intersect found");
        }
        IntersectNotFound(tip) => {
            tracing::info!(peer = %msg.peer, ?tip, "intersect not found");
            eff.send(&msg.handler, chainsync::InitiatorMessage::Done)
                .await;
        }
        RollForward(header_content, tip) => {
            let minted_header: MintedHeader<'_> =
                cbor::decode(header_content.cbor.as_slice()).unwrap();
            let header = Header::from(minted_header);
            let block_header = BlockHeader::from(header);
            let header_hash = block_header.hash();
            let store = Store::new(eff.clone());
            store.store_header(&block_header).unwrap();
            store.set_best_chain_hash(&header_hash).unwrap();

            tracing::info!(peer = %msg.peer, hash = header_hash.to_string(), ?tip, "roll forward");
            eff.send(&msg.handler, chainsync::InitiatorMessage::RequestNext)
                .await;
        }
        RollBackward(point, tip) => {
            tracing::info!(peer = %msg.peer, ?point, ?tip, "roll backward");
            eff.send(&msg.handler, chainsync::InitiatorMessage::RequestNext)
                .await;
        }
    }
}

/// This function can timeout the execution of a manager after a given duration.
async fn wait_for<F>(running: F, duration: Duration) -> anyhow::Result<()>
where
    F: IntoFuture,
{
    match timeout(duration, running).await {
        Ok(_) => anyhow::bail!("test should have timed out"),
        Err(_) => {
            tracing::info!("test timed out as expected");
        }
    };
    Ok(())
}

/// Resource type definitions
pub type ResourceBlockValidation = Arc<dyn CanValidateBlocks + Send + Sync>;
pub type ResourceHeaderValidation = Arc<dyn CanValidateHeaders + Send + Sync>;

/// Add resources for each role.
/// In particular set up an in-memory chain store with different chain lengths for the initiator and responder.
fn add_resources(
    network: &mut TokioBuilder,
    role: Role,
    connections: TokioConnections,
) -> anyhow::Result<()> {
    let chain_store = Arc::new(InMemConsensusStore::default());
    let mempool = Arc::new(InMemoryMempool::default());
    initialize_chain_store(chain_store.clone(), role)?;
    initialize_mempool(mempool.clone(), role)?;
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
    network.resources().put::<ResourceMempool<Tx>>(mempool);
    Ok(())
}

/// Initialize the chain store with a chain of headers.
/// The responder chain is longer than the initiator chain to force the initiator to catch up.
fn initialize_chain_store(
    chain_store: Arc<InMemConsensusStore<BlockHeader>>,
    role: Role,
) -> anyhow::Result<()> {
    // Use the same root header for both initiator and responder
    let origin_hash: HeaderHash = amaru_kernel::Hash::from_str(
        "4df4505d862586f9e2c533c5fbb659f04402664db1b095aba969728abfb77301",
    )?;
    let root_header = BlockHeader::from(make_header(0, 0, Some(origin_hash)));
    chain_store.set_anchor_hash(&root_header.hash())?;
    let chain_size = if role == Role::Responder { 5 } else { 2 };
    let mut headers = run(any_headers_chain_with_root(chain_size, root_header.hash()));
    headers.insert(0, root_header);

    for header in headers.iter() {
        chain_store.store_header(header)?;
        chain_store.roll_forward_chain(&header.point())?;
        chain_store.set_best_chain_hash(&header.hash())?;
    }
    Ok(())
}

const TXS_NB: u64 = 10;

fn initialize_mempool(mempool: Arc<InMemoryMempool<Tx>>, role: Role) -> anyhow::Result<()> {
    let tx_count = if role == Role::Initiator { TXS_NB } else { 0 };
    create_transactions_in_mempool(mempool.clone(), tx_count);
    Ok(())
}

/// By construction we return the same tx ids as the ones created in the function above
fn get_tx_ids() -> Vec<TxId> {
    create_transactions(TXS_NB)
        .into_iter()
        .map(|tx| TxId::from(&tx))
        .collect()
}
