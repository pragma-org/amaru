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

use crate::mempool_effects::MemoryPool;
use crate::{
    chainsync,
    chainsync::ChainSyncInitiatorMsg,
    manager,
    manager::{Manager, ManagerMessage},
    network_effects::{Network, NetworkOps},
    protocol::Role,
    store_effects::{ResourceHeaderStore, Store},
    tx_submission::{create_transactions, create_transactions_in_mempool},
};
use amaru_kernel::cardano::network_block::{NetworkBlock, make_network_block};
use amaru_kernel::{
    Block, BlockHeader, HeaderHash, IsHeader, NetworkMagic, NetworkName, Peer, Point, Transaction,
    any_headers_chain_with_root, cbor, make_header,
    utils::{string::ListToString, tests::run_strategy},
};
use amaru_mempool::InMemoryMempool;
use amaru_network::connection::TokioConnections;
use amaru_ouroboros_traits::{
    CanValidateBlocks, ChainStore, ConnectionProvider, ConnectionResource, ResourceMempool, TxId,
    TxSubmissionMempool,
    can_validate_blocks::{
        CanValidateHeaders,
        mock::{MockCanValidateBlocks, MockCanValidateHeaders},
    },
    in_memory_consensus_store::InMemConsensusStore,
};
use amaru_slot_arithmetic::EraHistory;
use pallas_primitives::babbage::{Header, MintedHeader};
use pure_stage::{
    Effects, StageGraph, StageRef, TryInStage,
    tokio::{TokioBuilder, TokioRunning},
};
use serde::{Deserialize, Serialize};
use std::{net::SocketAddr, str::FromStr, sync::Arc, time::Duration};
use tokio::runtime::Handle;
use tokio::sync::Notify;
use tracing_subscriber::EnvFilter;

/// This test simulates a connection between an initiator and a responder over TCP.
/// We want to observe that eventually the initiator catches up with the responder's chain
/// and that both nodes have the same blocks and transactions.
#[tokio::test]
async fn test_connect_initiator_responder() -> anyhow::Result<()> {
    setup_logging(true);
    let (responder, addr, responder_done) = start_responder().await?;
    let (initiator, initiator_done) = start_initiator(addr).await?;

    wait_for_termination(responder_done, initiator_done).await?;
    check_state(initiator, responder)?;
    Ok(())
}

/// Create and start a responder node that listens for incoming connections on a free port found at
/// runtime. Return the address it is listening on.
async fn start_responder() -> anyhow::Result<(TokioRunning, SocketAddr, Arc<Notify>)> {
    tracing::info!("Creating the responder");
    let mut responder_network = TokioBuilder::default();
    let era_history: &EraHistory = NetworkName::Preprod.into();
    let responder_manager = Manager::new(
        NetworkMagic::PREPROD,
        StageRef::blackhole(),
        Arc::new(era_history.clone()),
    );

    let responder_stage = responder_network.stage("responder", manager::stage);
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
    Ok((running_responder, peer_addr, notify))
}

/// Create and start an initiator node that connects to the given port.
/// ChainSync events are sent to a stage that stores them in the in-memory store without further processing.
async fn start_initiator(
    addr: impl Into<Option<SocketAddr>>,
) -> anyhow::Result<(TokioRunning, Arc<Notify>)> {
    tracing::info!("Creating the initiator");
    let addr = addr
        .into()
        .unwrap_or_else(|| SocketAddr::from(([127, 0, 0, 1], 3000)));
    let peer = Peer::from_addr(&addr);
    let mut initiator_network = TokioBuilder::default();

    // create stages
    let initiator_stage = initiator_network.stage("initiator", manager::stage);
    let chainsync_stage = initiator_network.stage("chainsync", test_chainsync_stage);

    // wire up stages
    let notify = Arc::new(Notify::new());
    let chainsync_stage = initiator_network.wire_up(
        chainsync_stage,
        ChainSyncStageState::new(initiator_stage.sender(), notify.clone()),
    );
    let era_history: &EraHistory = NetworkName::Preprod.into();
    let initiator_manager = Manager::new(
        NetworkMagic::PREPROD,
        chainsync_stage.without_state(),
        Arc::new(era_history.clone()),
    );
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

    Ok((running_initiator, notify))
}

/// Wait until both nodes signal that they are done.
/// We timeout after 5 seconds to avoid hanging tests. If the test times out it will fail later when checking the state.
async fn wait_for_termination(
    responder_done: Arc<Notify>,
    initiator_done: Arc<Notify>,
) -> anyhow::Result<()> {
    let _ = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(responder_done.notified(), initiator_done.notified());
    })
    .await;
    Ok(())
}

/// Verify that both nodes have the same state: same best chain, same blocks, same transactions.
fn check_state(initiator: TokioRunning, responder: TokioRunning) -> anyhow::Result<()> {
    let responder_chain_store = responder.resources().get::<ResourceHeaderStore>()?.clone();
    let responder_mempool = responder
        .resources()
        .get::<ResourceMempool<Transaction>>()?
        .clone();

    let initiator_chain_store = initiator.resources().get::<ResourceHeaderStore>()?.clone();
    let initiator_mempool = initiator
        .resources()
        .get::<ResourceMempool<Transaction>>()?
        .clone();

    // Verify that the 2 nodes have the same best chain
    assert_eq!(
        initiator_chain_store.get_best_chain_hash(),
        responder_chain_store.get_best_chain_hash()
    );

    // Verify that the 2 nodes have the same blocks headers
    // This makes it easier to spot differences in case of failure, before comparing full blocks.
    let initiator_block_headers = initiator_chain_store.retrieve_best_chain();
    let responder_block_headers = responder_chain_store.retrieve_best_chain();
    assert_eq!(initiator_block_headers, responder_block_headers);

    let initiator_blocks = get_blocks(initiator_chain_store);
    let responder_blocks = get_blocks(responder_chain_store);

    assert_eq!(
        initiator_blocks,
        responder_blocks,
        "initiator blocks {:?}\nresponder blocks {:?}",
        initiator_blocks
            .iter()
            .map(|b| b.0)
            .collect::<Vec<_>>()
            .list_to_string(","),
        responder_blocks
            .iter()
            .map(|b| b.0)
            .collect::<Vec<_>>()
            .list_to_string(",")
    );

    // Verify that the 2 nodes have the same transactions
    let tx_ids = get_tx_ids();
    assert_eq!(
        responder_mempool.get_txs_for_ids(tx_ids.as_slice()),
        initiator_mempool.get_txs_for_ids(tx_ids.as_slice())
    );
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct PullAccept;

#[derive(Debug, Deserialize, Serialize)]
pub struct AcceptState {
    manager_stage: StageRef<ManagerMessage>,
    #[serde(skip)]
    notify: Arc<Notify>,
}

impl PartialEq for AcceptState {
    fn eq(&self, other: &Self) -> bool {
        self.manager_stage == other.manager_stage
    }
}

impl Eq for AcceptState {}

impl AcceptState {
    fn new(manager_stage: StageRef<ManagerMessage>, notify: Arc<Notify>) -> Self {
        Self {
            manager_stage,
            notify,
        }
    }
}

/// Create a stage that accepts incoming connections and notifies the manager
/// about them. This can not be implemented using contramap because we need to
/// create a sender for that stage and this is not possible with an adapted stage.
pub async fn accept_stage(
    state: AcceptState,
    _msg: PullAccept,
    eff: Effects<PullAccept>,
) -> AcceptState {
    tracing::info!("pull accept");
    // Terminate if the mempool already has all expected transactions
    let mempool = MemoryPool::new(eff.clone());
    let expected_tx_ids = get_tx_ids();
    let txs = mempool.get_txs_for_ids(expected_tx_ids.as_slice());
    if txs.len() == expected_tx_ids.len() {
        tracing::info!("all txs retrieved, done");
        state.notify.notify_waiters();
    } else {
        tracing::info!(
            "still missing txs {}, continuing",
            expected_tx_ids.len() - txs.len()
        );
    }

    match Network::new(&eff).accept().await {
        Ok((peer, connection_id)) => {
            eff.send(
                &state.manager_stage,
                ManagerMessage::Accepted(peer, connection_id),
            )
            .await;
        }
        Err(err) => {
            tracing::error!(?err, "failed to accept a connection");
            return eff.terminate().await;
        }
    }
    eff.schedule_after(PullAccept, Duration::from_millis(100))
        .await;
    state
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

/// State for the ChainSync stage
/// The stage batches block fetch requests to test the manager's block fetch capabilities with the Message::RequestRange variant.
/// We accumulate the next points to fetch in this state and keep track of the total number of requested blocks.
#[derive(Debug, Serialize, Deserialize)]
struct ChainSyncStageState {
    manager: StageRef<ManagerMessage>,
    #[serde(skip)]
    notify: Arc<Notify>,
    blocks_to_fetch: Vec<Point>,
    total_requested_blocks: usize,
}

impl PartialEq for ChainSyncStageState {
    fn eq(&self, other: &Self) -> bool {
        self.manager == other.manager
            && self.blocks_to_fetch == other.blocks_to_fetch
            && self.total_requested_blocks == other.total_requested_blocks
    }
}

impl Eq for ChainSyncStageState {}

impl ChainSyncStageState {
    fn new(manager: StageRef<ManagerMessage>, notify: Arc<Notify>) -> Self {
        Self {
            manager,
            notify,
            blocks_to_fetch: Vec::new(),
            total_requested_blocks: 0,
        }
    }
}

/// This is a simplified version of the chain sync processing
/// that only stores headers and fetches blocks in batches of 3.
/// There is no validation or chain selection logic here.
async fn test_chainsync_stage(
    mut state: ChainSyncStageState,
    msg: ChainSyncInitiatorMsg,
    eff: Effects<ChainSyncInitiatorMsg>,
) -> ChainSyncStageState {
    use crate::chainsync::InitiatorResult::*;
    match msg.msg {
        Initialize => {
            tracing::info!(peer = %msg.peer,"initializing chainsync");
        }
        IntersectFound(point, tip) => {
            tracing::info!(peer = %msg.peer, %point, tip_point = %tip.point(), "intersect found");
        }
        IntersectNotFound(tip) => {
            tracing::info!(peer = %msg.peer, %tip, "intersect not found");
            eff.send(&msg.handler, chainsync::InitiatorMessage::Done)
                .await;
        }
        RollForward(header_content, tip) => {
            let minted_header: MintedHeader<'_> =
                cbor::decode(header_content.cbor.as_slice()).unwrap();
            let header = Header::from(minted_header);
            let block_header = BlockHeader::from(header);
            let header_hash = block_header.hash();
            let point = block_header.point();
            let store = Store::new(eff.clone());
            let peer = msg.peer;
            tracing::info!(%peer, hash = header_hash.to_string(), %tip, "roll forward");

            // store the header, update the best chain, fetch and store the block
            store.store_header(&block_header).unwrap();
            store.roll_forward_chain(&point).unwrap();
            store.set_best_chain_hash(&header_hash).unwrap();

            // We accumulate points to fetch and fetch them in batches of 3
            state.blocks_to_fetch.push(point);

            // By construction the initiator and the responder just have 1 block in common
            // so we know that we eventually need to fetch RESPONDER_BLOCKS_NB - 1 blocks.
            let remaining_number_of_blocks_to_retrieve =
                RESPONDER_BLOCKS_NB - 1 - state.total_requested_blocks;

            // If the last batch isn't full but would allow us to complete the retrieval, we fetch it as well.
            if state.blocks_to_fetch.len() == 3
                || state.blocks_to_fetch.len() == remaining_number_of_blocks_to_retrieve
            {
                let from = *state.blocks_to_fetch.first().unwrap();
                let through = *state.blocks_to_fetch.last().unwrap();
                let blocks = eff
                    .call(&state.manager, Duration::from_secs(1), move |cr| {
                        ManagerMessage::FetchBlocks {
                            peer,
                            from,
                            through,
                            cr,
                        }
                    })
                    .await
                    .or_terminate(&eff, async |e| {
                        tracing::error!("failed to fetch blocks: {e:?}")
                    })
                    .await;

                state.total_requested_blocks += state.blocks_to_fetch.len();

                // store the fetched blocks with their corresponding headers.
                tracing::info!("retrieved {} blocks", blocks.blocks.len());
                for network_block in blocks.blocks {
                    let block_header = network_block
                        .decode_header()
                        .expect("failed to extract header from block");
                    tracing::info!("storing block {:?}", block_header.point());
                    store
                        .store_block(&block_header.hash(), &network_block.raw_block())
                        .unwrap();
                }
                state.blocks_to_fetch.clear();
            };

            if state.total_requested_blocks == RESPONDER_BLOCKS_NB - 1 {
                tracing::info!("all blocks retrieved, done");
                state.notify.notify_waiters();
            } else {
                eff.send(&msg.handler, chainsync::InitiatorMessage::RequestNext)
                    .await;
            }
            return state;
        }
        RollBackward(point, tip) => {
            tracing::info!(peer = %msg.peer, %point, %tip, "roll backward");
            eff.send(&msg.handler, chainsync::InitiatorMessage::RequestNext)
                .await;
        }
    }
    state
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
    network
        .resources()
        .put::<ResourceMempool<Transaction>>(mempool);
    Ok(())
}

const RESPONDER_BLOCKS_NB: usize = 10;
const INITIATOR_BLOCKS_NB: usize = 3;

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
    let root_header = BlockHeader::from(make_header(100_000_000, 100_000_000, Some(origin_hash)));
    chain_store.set_anchor_hash(&root_header.hash())?;
    let chain_size = if role == Role::Responder {
        RESPONDER_BLOCKS_NB - 1
    } else {
        INITIATOR_BLOCKS_NB
    };
    let mut headers = run_strategy(any_headers_chain_with_root(chain_size, root_header.point()));
    headers.insert(0, root_header);

    for header in headers.iter() {
        chain_store.store_header(header)?;
        chain_store.roll_forward_chain(&header.point())?;
        chain_store.set_best_chain_hash(&header.hash())?;

        tracing::info!("storing block for header {}", header.point());
        let network_block = make_network_block(header);
        chain_store.store_block(&header.hash(), &network_block.raw_block())?;
    }
    Ok(())
}

/// Retrieve all blocks from the chain store starting from the best chain tip down to the root.
fn get_blocks(store: Arc<dyn ChainStore<BlockHeader>>) -> Vec<(HeaderHash, Block)> {
    store
        .retrieve_best_chain()
        .iter()
        .map(|h| {
            let b = store
                .load_block(h)
                .unwrap()
                .expect("missing block for best-chain header");
            (
                *h,
                NetworkBlock::try_from(b)
                    .expect("failed to decode raw block")
                    .decode_block()
                    .expect("failed to decode block"),
            )
        })
        .collect()
}

const TXS_NB: u64 = 10;

fn initialize_mempool(
    mempool: Arc<InMemoryMempool<Transaction>>,
    role: Role,
) -> anyhow::Result<()> {
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
