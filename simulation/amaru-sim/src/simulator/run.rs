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
use amaru_consensus::IsHeader;
use amaru_consensus::consensus::headers_tree::HeadersTree;
use amaru_consensus::consensus::select_chain::{DEFAULT_MAXIMUM_FRAGMENT_LENGTH, SelectChain};
use amaru_consensus::consensus::store::ChainStore;
use amaru_consensus::consensus::validate_header::ValidateHeader;
use amaru_consensus::consensus::{
    ChainSyncEvent, ValidateHeaderEvent, build_stage_graph, store_effects,
};
use amaru_kernel::Point::{Origin, Specific};
use amaru_kernel::network::NetworkName;
use amaru_kernel::peer::Peer;
use amaru_kernel::protocol_parameters::GlobalParameters;
use amaru_kernel::{Point, to_cbor};
use amaru_slot_arithmetic::Slot;
use amaru_stores::rocksdb::consensus::InMemConsensusStore;
use pallas_crypto::hash::Hash;
use pallas_primitives::babbage::Header;
use pure_stage::simulation::SimulationBuilder;
use pure_stage::trace_buffer::TraceBuffer;
use pure_stage::{Instant, Receiver, StageGraph, StageRef};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Runtime;
use tokio::sync::Mutex;
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
        async |(msg_id, downstream), msg: ValidateHeaderEvent, eff| {
            let (peer, chain_sync_message) = match msg {
                ValidateHeaderEvent::Validated { peer, header, .. } => (
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
                ),
                ValidateHeaderEvent::Rollback {
                    peer,
                    rollback_point,
                    ..
                } => (
                    peer,
                    ChainSyncMessage::Bck {
                        msg_id,
                        slot: rollback_point.slot_or_default(),
                        hash: Bytes {
                            bytes: Hash::from(&rollback_point).as_slice().to_vec(),
                        },
                    },
                ),
            };
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

    network.resources().put(chain_ref);
    network.resources().put(global_parameters);

    (receiver.without_state(), rx)
}

fn init_node(
    args: &Args,
) -> (
    GlobalParameters,
    SelectChain,
    ValidateHeader,
    store_effects::ResourceHeaderStore,
) {
    let network_name = NetworkName::Testnet(42);
    let global_parameters: &GlobalParameters = network_name.into();
    let stake_distribution: FakeStakeDistribution =
        FakeStakeDistribution::from_file(&args.stake_distribution_file, global_parameters).unwrap();

    let mut chain_store = InMemConsensusStore::new();

    populate_chain_store(
        &mut chain_store,
        &args.start_header,
        &args.consensus_context_file,
    )
    .unwrap_or_else(|e| panic!("cannot populate the chain store: {e:?}"));

    let select_chain = make_chain_selector(
        Origin,
        &chain_store,
        &(1..=args.number_of_upstream_peers)
            .map(|i| Peer::new(&format!("c{}", i)))
            .collect::<Vec<_>>(),
    );
    let chain_ref = Arc::new(Mutex::new(chain_store));
    let validate_header = ValidateHeader::new(Arc::new(stake_distribution));

    (
        global_parameters.clone(),
        select_chain,
        validate_header,
        chain_ref,
    )
}

fn make_chain_selector(
    tip: Point,
    chain_store: &impl ChainStore<Header>,
    peers: &Vec<Peer>,
) -> SelectChain {
    let root = match tip {
        Origin => None,
        Specific(..) => match chain_store.load_header(&Hash::from(&tip)) {
            None => panic!("Tip {:?} not found in chain store", tip),
            Some(header) => Some(header),
        },
    };

    let root_hash = root.as_ref().map(|r| r.hash()).unwrap_or(Origin.hash());
    let mut tree = HeadersTree::new(DEFAULT_MAXIMUM_FRAGMENT_LENGTH, &root);
    for peer in peers {
        tree.initialize_peer(peer, &root_hash)
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
