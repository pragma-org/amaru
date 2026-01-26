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
use crate::network_effects::{Network, NetworkOps};
use crate::protocol::Role;
use crate::store_effects::{ResourceHeaderStore, Store};
use crate::tx_submission::{create_transactions, create_transactions_in_mempool};
use crate::{chainsync, manager};
use amaru_kernel::is_header::tests::{any_headers_chain_with_root, make_header, run};
use amaru_kernel::peer::Peer;
use amaru_kernel::protocol_messages::network_magic::NetworkMagic;
use amaru_kernel::{BlockHeader, HeaderHash, IsHeader, Point, RawBlock, cbor, to_cbor};
use amaru_mempool::InMemoryMempool;
use amaru_network::connection::TokioConnections;
use amaru_ouroboros_traits::can_validate_blocks::CanValidateHeaders;
use amaru_ouroboros_traits::can_validate_blocks::mock::{
    MockCanValidateBlocks, MockCanValidateHeaders,
};
use amaru_ouroboros_traits::in_memory_consensus_store::InMemConsensusStore;
use amaru_ouroboros_traits::{
    CanValidateBlocks, ChainStore, ConnectionProvider, ConnectionResource, ResourceMempool, TxId,
};
use pallas_primitives::babbage::{Header, MintedHeader};
use pallas_primitives::conway::{Block, MintedBlock, Tx};
use pure_stage::tokio::{TokioBuilder, TokioRunning};
use pure_stage::{Effects, StageGraph, StageRef, TryInStage, Void};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::ops::Deref;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Handle;
use tokio::time::timeout;
use tracing_subscriber::EnvFilter;

/// This test simulates a connection between an initiator and a responder over TCP.
/// We want to observe that eventually the initiator catches up with the responder's chain
/// and that both nodes have the same blocks and transactions.
#[tokio::test]
async fn test_connect_initiator_responder() -> anyhow::Result<()> {
    setup_logging(true);
    let (responder, addr) = start_responder().await?;
    let initiator = start_initiator(addr).await?;

    wait_for_termination(&responder, &initiator).await?;
    check_state(initiator, responder)?;
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

    let accept_stage = responder_network.stage("accept", accept_stage);
    let accept_stage = responder_network.wire_up(accept_stage, responder_stage.without_state());
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

    // create stages
    let initiator_stage = initiator_network.stage("initiator", manager::stage);
    let chainsync_stage = initiator_network.stage("chainsync", test_chainsync_stage);

    // wire up stages
    let chainsync_stage = initiator_network.wire_up(
        chainsync_stage,
        ChainSyncStageState::new(initiator_stage.sender()),
    );
    let initiator_manager = Manager::new(NetworkMagic::PREPROD, chainsync_stage.without_state());
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

/// Wait for the initiator to terminate on its own when it has received all expected blocks.
/// Timeout after a few seconds if that's not the case
async fn wait_for_termination(
    initiator: &TokioRunning,
    responder: &TokioRunning,
) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(3), async {
        tokio::select! {
            _ = responder.clone().join() => (),
            _ = initiator.clone().join() => (),
        }
    })
    .await?;
    Ok(())
}

/// Verify that both nodes have the same state: same best chain, same blocks, same transactions.
fn check_state(initiator: TokioRunning, responder: TokioRunning) -> anyhow::Result<()> {
    let responder_chain_store = responder.resources().get::<ResourceHeaderStore>()?.clone();
    let responder_mempool = responder.resources().get::<ResourceMempool<Tx>>()?.clone();

    let initiator_chain_store = initiator.resources().get::<ResourceHeaderStore>()?.clone();
    let initiator_mempool = initiator.resources().get::<ResourceMempool<Tx>>()?.clone();

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
    assert_eq!(initiator_blocks, responder_blocks);

    // Verify that the 2 nodes have the same transactions
    let tx_ids = get_tx_ids();
    assert_eq!(
        responder_mempool.get_txs_for_ids(tx_ids.as_slice()),
        initiator_mempool.get_txs_for_ids(tx_ids.as_slice())
    );
    Ok(())
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

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct PullAccept;

/// Create a stage that accepts incoming connections and notifies the manager
/// about them. This can not be implemented using contramap because we need to
/// create a sender for that stage and this is not possible with an adapted stage.
pub async fn accept_stage(
    manager_stage: StageRef<ManagerMessage>,
    _msg: PullAccept,
    eff: Effects<PullAccept>,
) -> StageRef<ManagerMessage> {
    match Network::new(&eff).accept().await {
        Ok((peer, connection_id)) => {
            eff.send(
                &manager_stage,
                ManagerMessage::Accepted(peer, connection_id),
            )
            .await;
        }
        Err(err) => {
            tracing::error!(?err, "failed to accept a connection");
            return eff.terminate().await;
        }
    }
    eff.send(&eff.me(), PullAccept).await;
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

/// State for the ChainSync stage
/// The stage batches block fetch requests to test the manager's block fetch capabilities with the Message::RequestRange variant.
/// We accumulate the next points to fetch in this state and keep track of the total number of requested blocks.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct ChainSyncStageState {
    manager: StageRef<ManagerMessage>,
    blocks_to_fetch: Vec<Point>,
    total_requested_blocks: usize,
}

impl ChainSyncStageState {
    fn new(manager: StageRef<ManagerMessage>) -> Self {
        Self {
            manager,
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
            tracing::info!(peer = %msg.peer, %point, %tip, "intersect found");
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
                    .or_terminate(&eff, async |_| tracing::error!("failed to fetch blocks"))
                    .await;

                state.total_requested_blocks += state.blocks_to_fetch.len();
                // store the fetched blocks with their corresponding headers.
                for block in blocks.blocks {
                    // Check that the block can be decoded
                    let block: Block = cbor::decode(block.as_slice())
                        .expect("Failed to parse Conway3.block bytes");
                    let header_point = BlockHeader::from(block.header.clone()).point();
                    tracing::info!("storing block {:?}", header_point);
                    store
                        .store_block(
                            &header_point.hash(),
                            &RawBlock::from(to_cbor(&block).as_slice()),
                        )
                        .unwrap();
                }
                state.blocks_to_fetch.clear();
            };

            if state.total_requested_blocks == RESPONDER_BLOCKS_NB - 1 {
                tracing::info!("all blocks retrieved, done");
                eff.terminate::<Void>().await;
                // eff.send(&msg.handler, chainsync::InitiatorMessage::Done)
                //     .await;
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
    let root_header = BlockHeader::from(make_header(0, 0, Some(origin_hash)));
    chain_store.set_anchor_hash(&root_header.hash())?;
    let chain_size = if role == Role::Responder {
        RESPONDER_BLOCKS_NB - 1
    } else {
        INITIATOR_BLOCKS_NB
    };
    let mut headers = run(any_headers_chain_with_root(chain_size, root_header.hash()));
    headers.insert(0, root_header);

    for (i, header) in headers.iter().enumerate() {
        chain_store.store_header(header)?;
        chain_store.roll_forward_chain(&header.point())?;
        chain_store.set_best_chain_hash(&header.hash())?;

        // Add a block for each header
        // We skip one block to test that the initiator can try to fetch missing blocks
        if i != 4 {
            tracing::info!("storing block for header {}", header.point());

            let mut block = make_block();
            block.header = header.header().clone();
            chain_store.store_block(&header.hash(), &RawBlock::from(to_cbor(&block).as_slice()))?;
        }
    }
    Ok(())
}

/// Retrieve all blocks from the chain store starting from the best chain tip down to the root.
fn get_blocks(store: Arc<dyn ChainStore<BlockHeader>>) -> Vec<Block> {
    store
        .retrieve_best_chain()
        .iter()
        .filter_map(|h| {
            store
                .load_block(h)
                .ok()
                .map(|b| cbor::decode(b.deref()).unwrap())
        })
        .collect()
}

fn make_block() -> Block {
    // These bytes are Conway3.block from Pallas https://github.com/txpipe/pallas/blob/main/test_data/conway3.block
    let bytes = hex::decode("820785828a1a00153df41a01aa8a0458201bbf3961f179735b68d8f85bcff85b1eaaa6ec3fa6218e4b6f4be7c6129e37ba5820472a53a312467a3b66ede974399b40d1ea428017bc83cf9647d421b21d1cb74358206ee6456894a5931829207e497e0be77898d090d0ac0477a276712dee34e51e05825840d35e871ff75c9a243b02c648bccc5edf2860edba0cc2014c264bbbdb51b2df50eff2db2da1803aa55c9797e0cc25bdb4486a4059c4687364ad66ed15b4ec199f58508af7f535948fac488dc74123d19c205ea2b02cbbf91104bbad140d4ba4bb4d75f7fdb762586802f116bdba3ecaa0840614a2b96d619006c3274b590bcd2599e39a17951cbc3db6348fa2688158384f081901965820d8038b5679ffc770b060578bcd7b33045f2c3aa5acc7bd8cde8b705cfe673d7584582030449be32ae7b8363fde830fc9624945862b281e481ec7f5997c75d1f2316c560018ca5840f5d96ce2055a67709c8e6809c882f71ebd7fc6350018d36d803a55b9230ec6c4cbcd41a09255db45214e278f89b39005ac0f213473acbf455165cdcaa9558e0c8209005901c02ba5dda40daa84b3f9c524016c21d7ce13f585062e35298aa31ea590fee809e75ae999dff9b3ee188e01cfcecc384faba50ca673af2388c3cf7407206019920e99e195bc8e6d1a42ef2b7fb549a8da0591180da17db7a24334b098bfef839334761ec51c2bd8a044fd1785b4e216f811dbdcba63eb853a477d3ea87a3b2d61ccfeae74765c51ec1313ffb121573bae4fc3a742825168760f615a0b2b6ef8a42084f9465501774310772de17a574d8d6bef6b14f4277c8b792b4f60f6408262e7aee5e95b8539df07f953d16b209b6d8fa598a6c51ab90659523720c98ffd254bf305106c0b9c6938c33323e191b5afbad8939270c76a82dc2124525aab11396b9de746be6d7fae2c1592c6546474cebe07d1f48c05f36f762d218d9d2ca3e67c27f0a3d82cdd1bab4afa7f3f5d3ecb10c6449300c01b55e5d83f6cefc6a12382577fc7f3de09146b5f9d78f48113622ee923c3484e53bff74df65895ec0ddd43bc9f00bf330681811d5d20d0e30eed4e0d4cc2c75d1499e05572b13fb4e7b0dabf6e36d1988b47fbdecffc01316885f802cd6c60e044bf50a15418530d628cffd506d4eb0db6155be94ce84fbf6529ee06ec78e9c3009c0f5504978dd150926281a400d90102828258202e6b2226fd74ab0cadc53aaa18759752752bd9b616ea48c0e7b7be77d1af4bf400825820d5dc99581e5f479d006aca0cd836c2bb7ddcd4a243f8e9485d3c969df66462cb00018182583900bbe56449ba4ee08c471d69978e01db384d31e29133af4546e6057335061771ead84921c0ca49a4b48ab03c2ad1b45a182a46485ed1c965411b0000000ba4332169021a0002c71d14d9010281841b0000000ba43b7400581de0061771ead84921c0ca49a4b48ab03c2ad1b45a182a46485ed1c965418400f6a2001bffffffffffffffff09d81e821bfffffffffffffffe1bfffffffffffffffff68275687474703a2f2f636f73746d646c732e74657374735820931f1d8cdfdc82050bd2baadfe384df8bf99b00e36cb12bfb8795beab3ac7fe581a100d9010281825820794ff60d3c35b97f55896d1b2a455fe5e89b77fb8094d27063ff1f260d21a67358403894a10bf9fca0592391cdeabd39891fc2f960fae5a2743c73391c495dfdf4ba4f1cb5ede761bebd7996eba6bbe4c126bcd1849afb9504f4ae7fb4544a93ff0ea080").expect("Failed to decode Conway3.block hex");
    let (_, block): (u16, MintedBlock<'_>) =
        cbor::decode(bytes.as_slice()).expect("Failed to parse Conway3.block bytes");
    Block::from(block)
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
