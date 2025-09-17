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
use amaru_consensus::consensus::effects::block_effects::ResourceBlockFetcher;
use amaru_consensus::consensus::effects::store_effects::{
    ResourceHeaderStore, ResourceHeaderValidation, ResourceParameters,
};
use amaru_consensus::consensus::errors::ConsensusError;
use amaru_consensus::consensus::events::{BlockValidationResult, ChainSyncEvent};
use amaru_consensus::consensus::headers_tree::HeadersTree;
use amaru_consensus::consensus::stages::fetch_block::BlockFetcher;
use amaru_consensus::consensus::stages::select_chain::{
    DEFAULT_MAXIMUM_FRAGMENT_LENGTH, SelectChain,
};
use amaru_consensus::consensus::stages::validate_block::ResourceBlockValidation;
use amaru_kernel::network::NetworkName;
use amaru_kernel::peer::Peer;
use amaru_kernel::protocol_parameters::GlobalParameters;
use amaru_kernel::{Point, to_cbor};
use amaru_ouroboros::can_validate_blocks::mock::MockCanValidateBlocks;
use amaru_ouroboros::in_memory_consensus_store::InMemConsensusStore;
use amaru_ouroboros::{ChainStore, IsHeader};
use amaru_slot_arithmetic::Slot;
use async_trait::async_trait;
use pallas_crypto::hash::Hash;
use pallas_primitives::babbage::Header;
use pure_stage::simulation::SimulationBuilder;
use pure_stage::trace_buffer::TraceBuffer;
use pure_stage::{Instant, Receiver, StageGraph, StageRef};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Runtime;
use tracing::{Span, info};

/// Run the full simulation:
///
/// * Create a simulation environment.
/// * Run the simulation.
pub fn run(rt: Runtime, args: Args) {
    let trace_buffer = Arc::new(parking_lot::Mutex::new(TraceBuffer::new(42, 1_000_000_000)));

    let spawn = || {
        let mut network = SimulationBuilder::default().with_trace_buffer(trace_buffer.clone());
        let (input, output) = spawn_node(args.clone(), &mut network);
        let running = network.run(rt.handle().clone());
        NodeHandle::from_pure_stage(input, output, running).unwrap()
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
///  * A handle to receive messages from the node
///
///
fn spawn_node(
    args: Args,
    network: &mut SimulationBuilder,
) -> (
    StageRef<Envelope<ChainSyncMessage>>,
    Receiver<Envelope<ChainSyncMessage>>,
) {
    info!("Spawning node!");

    let (global_parameters, select_chain, validate_header, chain_ref) = init_node(&args);

    let receiver = network.stage(
        "receiver",
        async |(downstream, output), msg: Envelope<ChainSyncMessage>, eff| {
            match msg.body {
                ChainSyncMessage::Init { msg_id, .. } => {
                    eff.send(
                        &output,
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

    let propagate_header_stage = network.stage(
        "propagate_header",
        async |(msg_id, downstream), msg: BlockValidationResult, eff| {
            if let Some((peer, chain_sync_message)) = match msg {
                BlockValidationResult::BlockValidated { peer, header, .. } => Some((
                    peer,
                    ChainSyncMessage::Fwd {
                        msg_id,
                        slot: header.point().slot_or_default(),
                        hash: Bytes {
                            bytes: Hash::from(&header.point()).as_slice().to_vec(),
                        },
                        header: Bytes {
                            bytes: to_cbor(&header),
                        },
                    },
                )),
                BlockValidationResult::RolledBackTo {
                    peer,
                    rollback_point,
                    ..
                } => Some((
                    peer,
                    ChainSyncMessage::Bck {
                        msg_id,
                        slot: rollback_point.slot_or_default(),
                        hash: Bytes {
                            bytes: Hash::from(&rollback_point).as_slice().to_vec(),
                        },
                    },
                )),
                BlockValidationResult::BlockValidationFailed { .. } => None,
            } {
                eff.send(
                    &downstream,
                    Envelope {
                        // FIXME: do we have the name of the node stored somewhere?
                        src: "n1".to_string(),
                        // XXX: this should be broadcast to ALL followers
                        dest: peer.name,
                        body: chain_sync_message,
                    },
                )
                .await;
            }
            (msg_id + 1, downstream)
        },
    );

    let receive_header_ref = build_stage_graph(
        &global_parameters,
        validate_header,
        select_chain,
        network,
        propagate_header_stage.sender(),
    );

    let (output, rx) = network.output("output", 10);
    let receiver = network.wire_up(receiver, (receive_header_ref, output.clone()));
    network.wire_up(propagate_header_stage, (0, output));

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

    (receiver.without_state(), rx)
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
