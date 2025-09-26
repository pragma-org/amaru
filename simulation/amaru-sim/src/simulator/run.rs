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

use crate::echo::Envelope;
use crate::simulator::bytes::Bytes;
use crate::simulator::generate::generate_entries;
use crate::simulator::ledger::{FakeStakeDistribution, populate_chain_store};
use crate::simulator::simulate::simulate;
use crate::simulator::{Args, Chain, History, NodeHandle, SimulateConfig};
use crate::sync::ChainSyncMessage;
use amaru::stages::build_stage_graph::build_stage_graph;
use amaru_consensus::consensus::effects::{
    ForwardEvent, ForwardEventListener, ResourceForwardEventListener, ResourceHeaderStore,
    ResourceHeaderValidation, ResourceParameters,
};
use amaru_consensus::consensus::effects::{ResourceBlockFetcher, ResourceBlockValidation};
use amaru_consensus::consensus::errors::ConsensusError;
use amaru_consensus::consensus::events::ChainSyncEvent;
use amaru_consensus::consensus::headers_tree::HeadersTree;
use amaru_consensus::consensus::stages::fetch_block::BlockFetcher;
use amaru_consensus::consensus::stages::select_chain::{
    DEFAULT_MAXIMUM_FRAGMENT_LENGTH, SelectChain,
};
use amaru_consensus::consensus::tip::HeaderTip;
use amaru_kernel::network::NetworkName;
use amaru_kernel::peer::Peer;
use amaru_kernel::protocol_parameters::GlobalParameters;
use amaru_kernel::{Point, to_cbor};
use amaru_ouroboros::can_validate_blocks::mock::MockCanValidateBlocks;
use amaru_ouroboros::in_memory_consensus_store::InMemConsensusStore;
use amaru_ouroboros::{ChainStore, IsHeader};
use amaru_slot_arithmetic::Slot;
use async_trait::async_trait;
use pallas_primitives::babbage::Header;
use pure_stage::simulation::SimulationBuilder;
use pure_stage::trace_buffer::TraceBuffer;
use pure_stage::{Instant, Receiver, StageGraph, StageRef};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::runtime::Runtime;
use tokio::sync::mpsc;
use tracing::{Span, info};

/// Run the full simulation:
///
/// * Create a simulation environment.
/// * Run the simulation.
pub fn run(rt: Runtime, args: Args) {
    let trace_buffer = Arc::new(parking_lot::Mutex::new(TraceBuffer::new(42, 1_000_000_000)));

    let spawn = |node_id: String| {
        let mut network = SimulationBuilder::default().with_trace_buffer(trace_buffer.clone());
        let (input, init_messages, output) = spawn_node(node_id, args.clone(), &mut network);
        let running = network.run(rt.handle().clone());
        NodeHandle::from_pure_stage(input, init_messages, output, running).unwrap()
    };

    let config = SimulateConfig::from(args.clone());
    simulate(
        &config,
        spawn,
        generate_entries(
            &config,
            &args.block_tree_file,
            Instant::at_offset(Duration::from_secs(0)),
            200.0,
        ),
        chain_property(&args.block_tree_file),
        trace_buffer.clone(),
        args.persist_on_success,
    )
    .unwrap_or_else(|e| panic!("{e}"));
}

/// Create and start a node
/// Return:
///
///  * A handle to send messages to the node
///  * A handle to receive init messages executed on the node
///  * A handle to receive output messages from the node
///
fn spawn_node(
    node_id: String,
    args: Args,
    network: &mut SimulationBuilder,
) -> (
    StageRef<Envelope<ChainSyncMessage>>,
    Receiver<Envelope<ChainSyncMessage>>,
    Receiver<Envelope<ChainSyncMessage>>,
) {
    info!("Spawning node!");
    let config = SimulateConfig::from(args.clone());

    let (global_parameters, select_chain, validate_header, chain_ref) = init_node(&args);

    // The receiver replies ok to init messages from the sender (via 'output', the only output of the graph)
    // and forwards chain sync messages to the rest of the processing graph
    let receiver = network.stage(
        "receiver",
        async |(downstream, output), msg: Envelope<ChainSyncMessage>, eff| {
            match msg.body {
                ChainSyncMessage::Init { msg_id, .. } => {
                    eff.send(
                        &output,
                        // Reply with InitOk
                        Envelope {
                            src: msg.dest,
                            dest: msg.src,
                            body: ChainSyncMessage::InitOk {
                                in_reply_to: msg_id,
                            },
                        },
                    )
                    .await
                }
                ChainSyncMessage::InitOk { .. } => (),
                ChainSyncMessage::Fwd {
                    slot, hash, header, ..
                } => {
                    eff.send(
                        &downstream,
                        ChainSyncEvent::RollForward {
                            peer: Peer::new(&msg.src),
                            point: Point::Specific(slot.into(), hash.into()),
                            raw_header: header.into(),
                            span: Span::current(),
                        },
                    )
                    .await
                }
                ChainSyncMessage::Bck { slot, hash, .. } => {
                    eff.send(
                        &downstream,
                        ChainSyncEvent::Rollback {
                            peer: Peer::new(&msg.src),
                            rollback_point: Point::Specific(slot.into(), hash.into()),
                            span: Span::current(),
                        },
                    )
                    .await
                }
            }
            (downstream, output)
        },
    );

    let our_tip = HeaderTip::new(Point::Origin, 0);
    let receive_header_ref = build_stage_graph(
        &global_parameters,
        validate_header,
        select_chain,
        our_tip,
        network,
    );

    let (output, rx1) = network.output("output", 10);
    let (sender, rx2) = mpsc::channel(10);
    let listener =
        MockForwardEventListener::new(node_id, config.number_of_downstream_peers, sender);

    let receiver = network.wire_up(receiver, (receive_header_ref, output.clone()));

    network.resources().put::<ResourceHeaderStore>(chain_ref);
    network
        .resources()
        .put::<ResourceParameters>(global_parameters);
    network
        .resources()
        .put::<ResourceBlockFetcher>(Arc::new(FakeBlockFetcher));
    network
        .resources()
        .put::<ResourceBlockValidation>(Arc::new(MockCanValidateBlocks));
    network
        .resources()
        .put::<ResourceForwardEventListener>(Arc::new(listener));

    (receiver.without_state(), rx1, Receiver::new(rx2))
}

fn init_node(
    args: &Args,
) -> (
    GlobalParameters,
    SelectChain,
    ResourceHeaderValidation,
    ResourceHeaderStore,
) {
    let network_name = NetworkName::Testnet(42);
    let global_parameters: &GlobalParameters = network_name.into();
    let stake_distribution: FakeStakeDistribution =
        FakeStakeDistribution::from_file(&args.stake_distribution_file, global_parameters).unwrap();

    let chain_store = Arc::new(InMemConsensusStore::new());

    populate_chain_store(
        chain_store.clone(),
        &args.start_header,
        &args.consensus_context_file,
    )
    .unwrap_or_else(|e| panic!("cannot populate the chain store: {e:?}"));

    let select_chain = make_chain_selector(
        chain_store.clone(),
        &(1..=args.number_of_upstream_peers)
            .map(|i| Peer::new(&format!("c{}", i)))
            .collect::<Vec<_>>(),
    );

    (
        global_parameters.clone(),
        select_chain,
        Arc::new(stake_distribution),
        chain_store,
    )
}

fn make_chain_selector(chain_store: Arc<dyn ChainStore<Header>>, peers: &Vec<Peer>) -> SelectChain {
    let mut tree = HeadersTree::new(chain_store.clone(), DEFAULT_MAXIMUM_FRAGMENT_LENGTH);
    for peer in peers {
        tree.initialize_peer(peer, &chain_store.get_anchor_hash())
            .expect("the root node is guaranteed to already be in the tree")
    }
    SelectChain::new(tree, peers)
}

/// Property: at the end of the simulation, the tip of the chain from the last block must
/// match the tip of the chain returned by the history of messages sent by the nodes under test.
fn chain_property(
    chain_data_path: &PathBuf,
) -> impl Fn(&History<ChainSyncMessage>) -> Result<(), String> + use<'_> {
    move |history| {
        // Get the tip of the chain from the chain data file
        // and compare it to the last entry in the history
        let blocks =
            Chain::read_blocks_from_file(chain_data_path).map_err(|err| err.to_string())?;
        let expected = blocks
            .last()
            .map(|block| (block.hash.clone(), Slot::from(block.slot)))
            .expect("empty chain data");

        if let Some(Envelope {
            body: ChainSyncMessage::Fwd { hash, slot, .. },
            ..
        }) = history.0.last()
        {
            let actual = (hash.clone(), *slot);
            if actual != expected {
                Err(format!(
                    "tip of chains don't match, expected:\n    {:?}\n  got:\n    {:?}",
                    expected, actual
                ))
            } else {
                Ok(())
            }
        } else {
            Err("impossible, no first entry in history".to_string())
        }
    }
}

/// A fake block fetcher that always returns an empty block.
/// This is used in for simulating the network.
#[derive(Clone, Debug, Default)]
pub struct FakeBlockFetcher;

#[async_trait]
impl BlockFetcher for FakeBlockFetcher {
    async fn fetch_block(&self, _peer: &Peer, _point: &Point) -> Result<Vec<u8>, ConsensusError> {
        Ok(vec![])
    }
}

/// This implementation of ForwardEventListener sends the received events a Sender that can collect them
/// to check them later.
///
/// A message id is maintained for each received event and each message is duplicated to the number of downstream peers.
#[derive(Clone, Debug)]
pub struct MockForwardEventListener {
    node_id: String,
    number_of_downstream_peers: u8,
    sender: mpsc::Sender<Envelope<ChainSyncMessage>>,
    msg_id: Arc<AtomicU64>,
}

impl MockForwardEventListener {
    pub fn new(
        node_id: String,
        number_of_downstream_peers: u8,
        sender: mpsc::Sender<Envelope<ChainSyncMessage>>,
    ) -> Self {
        Self {
            node_id,
            number_of_downstream_peers,
            msg_id: Arc::new(AtomicU64::new(0)),
            sender,
        }
    }
}

#[async_trait]
impl ForwardEventListener for MockForwardEventListener {
    async fn send(&self, event: ForwardEvent) -> anyhow::Result<()> {
        fn message(event: &ForwardEvent, msg_id: u64) -> ChainSyncMessage {
            match event {
                ForwardEvent::Forward(header) => ChainSyncMessage::Fwd {
                    msg_id,
                    slot: header.point().slot_or_default(),
                    hash: Bytes {
                        bytes: header.hash().as_slice().to_vec(),
                    },
                    header: Bytes {
                        bytes: to_cbor(&header),
                    },
                },
                ForwardEvent::Backward(tip) => ChainSyncMessage::Bck {
                    msg_id,
                    slot: tip.point().slot_or_default(),
                    hash: Bytes {
                        bytes: tip.hash().as_slice().to_vec(),
                    },
                },
            }
        }

        // This allocates a range of message ids from
        // self.msg_id to self.msg_id + number_of_downstream_peers
        let base_msg_id = self
            .msg_id
            .fetch_add(self.number_of_downstream_peers as u64, Ordering::Relaxed);

        for i in 1..=self.number_of_downstream_peers {
            let dest = format!("c{}", i);
            let msg_id = base_msg_id + i as u64;
            println!("msg id {}", msg_id);
            let envelope = Envelope {
                src: self.node_id.clone(),
                dest,
                body: message(&event, msg_id),
            };
            self.sender.send(envelope).await?;
        }
        Ok(())
    }
}
