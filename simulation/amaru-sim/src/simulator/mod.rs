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

use super::echo::Envelope;
use amaru_consensus::{
    IsHeader,
    consensus::{
        ChainSyncEvent, DecodedChainSyncEvent, ValidateHeaderEvent, build_stage_graph,
        headers_tree::HeadersTree,
        receive_header::handle_chain_sync,
        select_chain::{DEFAULT_MAXIMUM_FRAGMENT_LENGTH, SelectChain},
        store::ChainStore,
        store_header::StoreHeader,
        validate_header::{
            ValidateHeader, ValidateHeaderResourceParameters, ValidateHeaderResourceStore,
        },
    },
};
use amaru_kernel::{
    Hash, Header,
    Point::{self, *},
    Slot,
    network::NetworkName,
    peer::Peer,
    protocol_parameters::GlobalParameters,
    to_cbor,
};
use amaru_stores::rocksdb::consensus::InMemConsensusStore;
use bytes::Bytes;
use clap::Parser;
use generate::{generate_entries, parse_json, read_chain_json};
use ledger::{FakeStakeDistribution, populate_chain_store};
use pure_stage::{
    Instant, Receiver, StageGraph, StageRef, Void, simulation::SimulationBuilder,
    trace_buffer::TraceBuffer,
};
use rand::Rng;
use simulate::{History, SimulateConfig, pure_stage_node_handle, simulate};
use std::{path::PathBuf, sync::Arc, time::Duration};
pub use sync::*;
use tokio::sync::Mutex;
use tracing::{Span, info};

mod bytes;
pub mod generate;
mod ledger;
pub mod shrink;
pub mod simulate;
mod sync;

#[derive(Debug, Parser, Clone)]
#[clap(name = "Amaru Simulator")]
#[clap(bin_name = "amaru-sim")]
#[clap(author, version, about, long_about = None)]
pub struct Args {
    /// Path of JSON-formatted stake distribution file.
    #[arg(long, default_value = "./stake_distribution.json")]
    pub stake_distribution_file: PathBuf,

    /// Path of JSON-formatted consensus context file.
    #[arg(long, default_value = "./consensus_context.json")]
    pub consensus_context_file: PathBuf,

    /// Path of the chain on-disk storage.
    #[arg(long, default_value = "./chain.db/")]
    pub chain_dir: PathBuf,

    /// Generated "block tree" file in JSON
    #[arg(long, default_value = "./chain.json")]
    pub block_tree_file: PathBuf,

    /// Starting point for the (simulated) chain.
    /// Default to genesis hash, eg. all-zero hash.
    #[arg(long, default_value_t = Hash::from([0; 32]))]
    pub start_header: Hash<32>,

    /// Number of tests to run in simulation
    #[arg(long, default_value = "50")]
    pub number_of_tests: Option<u32>,

    /// Number of nodes in simulation.
    #[arg(long, default_value = "1")]
    pub number_of_nodes: Option<u8>,

    /// Number of upstream peers to simulate
    #[arg(long, default_value = "2")]
    pub number_of_upstream_peers: Option<u8>,

    #[arg(long)]
    pub disable_shrinking: bool,

    /// Seed for simulation testing.
    #[arg(long)]
    pub seed: Option<u64>,

    /// Persist pure-stage's effect trace aka schedule even if the test passes.
    #[arg(long)]
    pub persist_on_success: bool,
}

fn init_node(
    args: &Args,
) -> (
    GlobalParameters,
    SelectChain,
    ValidateHeader,
    ValidateHeaderResourceStore,
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

    let select_chain = SelectChain::new(make_chain_selector(
        Origin,
        &chain_store,
        &(1..=args.number_of_upstream_peers.unwrap_or(2))
            .map(|i| Peer::new(&format!("c{}", i)))
            .collect::<Vec<_>>(),
    ));
    let chain_ref = Arc::new(Mutex::new(chain_store));
    let validate_header = ValidateHeader::new(Arc::new(stake_distribution));

    (
        global_parameters.clone(),
        select_chain,
        validate_header,
        chain_ref,
    )
}

fn spawn_node(
    args: Args,
    network: &mut SimulationBuilder,
) -> (
    Receiver<Envelope<ChainSyncMessage>>,
    StageRef<Envelope<ChainSyncMessage>, Void>,
) {
    info!("Spawning node!");

    let (global_parameters, select_chain, validate_header, chain_ref) = init_node(&args);

    let init_store = StoreHeader {
        store: chain_ref.clone(),
    };

    let receive_stage = network.stage(
        "receive_header",
        async |(_state, downstream, out), msg: Envelope<ChainSyncMessage>, eff| {
            match msg.body {
                ChainSyncMessage::Init { msg_id, .. } => {
                    let reply_msg = ChainSyncMessage::InitOk {
                        in_reply_to: msg_id,
                    };
                    let reply = Envelope {
                        src: msg.dest,
                        dest: msg.src,
                        body: reply_msg,
                    };
                    eff.send(&out, reply).await
                }
                ChainSyncMessage::InitOk { .. } => (),
                ChainSyncMessage::Fwd {
                    slot, hash, header, ..
                } => {
                    let decoded = match handle_chain_sync(ChainSyncEvent::RollForward {
                        peer: Peer::new(&msg.src),
                        point: Point::Specific(slot.into(), hash.into()),
                        raw_header: header.into(),
                        span: Span::current(),
                    }) {
                        Ok(decoded) => decoded,
                        Err(error) => {
                            tracing::error!(peer = %msg.src, %error, "invalid roll forward");
                            return eff.terminate().await;
                        }
                    };
                    eff.send(&downstream, decoded).await
                }
                ChainSyncMessage::Bck { slot, hash, .. } => {
                    let decoded = match handle_chain_sync(ChainSyncEvent::Rollback {
                        peer: Peer::new(&msg.src),
                        rollback_point: Point::Specific(slot.into(), hash.into()),
                        span: Span::current(),
                    }) {
                        Ok(decoded) => decoded,
                        Err(error) => {
                            tracing::error!(peer = %msg.src, %error, "invalid rollback");
                            return eff.terminate().await;
                        }
                    };
                    eff.send(&downstream, decoded).await
                }
            };
            ((), downstream, out)
        },
    );

    let store_header_stage = network.stage(
        "store_header",
        async |(store, downstream): (StoreHeader, _), msg: DecodedChainSyncEvent, eff| {
            let peer = msg.peer();
            let result = match store.handle_event(msg).await {
                Ok(result) => result,
                Err(error) => {
                    tracing::error!(%peer, %error, "invalid header");
                    return eff.terminate().await;
                }
            };
            eff.send(&downstream, result).await;
            (store, downstream)
        },
    );

    let select_chain_stage = network.stage(
        "select_chain",
        async |(mut select_chain, downstream): (SelectChain, _),
               msg: DecodedChainSyncEvent,
               eff| {
            match msg {
                DecodedChainSyncEvent::RollForward {
                    peer, header, span, ..
                } => {
                    let result = match select_chain.select_chain(peer.clone(), header, span).await {
                        Ok(result) => result,
                        Err(error) => {
                            tracing::error!(%peer, %error, "invalid header");
                            return eff.terminate().await;
                        }
                    };
                    eff.send(&downstream, result).await;
                }
                DecodedChainSyncEvent::Rollback {
                    peer,
                    rollback_point,
                    span,
                    ..
                } => {
                    let result = match select_chain
                        .select_rollback(peer.clone(), rollback_point, span)
                        .await
                    {
                        Ok(result) => result,
                        Err(error) => {
                            tracing::error!(%peer, %error, "invalid rollback");
                            return eff.terminate().await;
                        }
                    };
                    eff.send(&downstream, result).await;
                }
            }
            (select_chain, downstream)
        },
    );

    let propagate_header_stage = network.stage(
        "propagate_header",
        async |(next_msg_id, downstream), msgs: Vec<ValidateHeaderEvent>, eff| {
            let mut msg_id = next_msg_id;
            for msg in msgs {
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
                msg_id += 1;
            }
            (msg_id, downstream)
        },
    );

    let validate_header_ref = build_stage_graph(
        &global_parameters,
        validate_header,
        network,
        select_chain_stage.sender(),
    );

    let (output, rx) = network.output("output", 10);
    let receive = network.wire_up(
        receive_stage,
        ((), store_header_stage.sender(), output.clone()),
    );
    network.wire_up(store_header_stage, (init_store, validate_header_ref));
    network.wire_up(
        select_chain_stage,
        (select_chain.clone(), propagate_header_stage.sender()),
    );
    network.wire_up(propagate_header_stage, (0, output.without_state()));

    network
        .resources()
        .put::<ValidateHeaderResourceStore>(chain_ref);
    network
        .resources()
        .put::<ValidateHeaderResourceParameters>(global_parameters);

    (rx, receive.without_state())
}

pub fn run(rt: tokio::runtime::Runtime, args: Args) {
    let number_of_tests = args.number_of_tests.unwrap_or(50);
    let number_of_nodes = args.number_of_nodes.unwrap_or(1);
    let number_of_upstream_peers = args.number_of_upstream_peers.unwrap_or(2);
    let disable_shrinking = args.disable_shrinking;
    let trace_buffer = Arc::new(parking_lot::Mutex::new(TraceBuffer::new(42, 1_000_000_000)));

    let spawn = || {
        let mut network = SimulationBuilder::default().with_trace_buffer(trace_buffer.clone());
        let (rx, receive) = spawn_node(args.clone(), &mut network);
        let running = network.run(rt.handle().clone());
        pure_stage_node_handle(rx, receive, running).unwrap()
    };

    let seed = args.seed.unwrap_or({
        let mut rng = rand::rng();
        rng.random::<u64>()
    });

    simulate(
        SimulateConfig {
            number_of_tests,
            seed,
            number_of_nodes,
            disable_shrinking,
        },
        spawn,
        generate_entries(
            &args.block_tree_file,
            Instant::at_offset(Duration::from_secs(0)),
            200.0,
            number_of_upstream_peers,
        ),
        chain_property(&args.block_tree_file),
        trace_buffer.clone(),
        args.persist_on_success,
    );
}

fn chain_property(
    chain_data_path: &PathBuf,
) -> impl Fn(&History<ChainSyncMessage>) -> Result<(), String> + use<'_> {
    move |history| {
        match history.0.last() {
            None => Err("impossible, no last entry in history".to_string()),
            Some(entry) => {
                // FIXME: the property is wrong, we should check the property
                // that the output message history is a prefix of the read chain
                let data = read_chain_json(chain_data_path);
                let blocks = parse_json(data.as_bytes()).map_err(|err| err.to_string())?;
                match &entry.body {
                    ChainSyncMessage::Fwd { hash, slot, .. } => {
                        let actual = (hash.clone(), *slot);
                        let expected = blocks
                            .last()
                            .map(|block| (block.hash.clone(), Slot::from(block.slot)))
                            .expect("empty chain data");
                        if actual != expected {
                            return Err(format!(
                                "tip of chains don't match, expected:\n    {:?}\n  got:\n    {:?}",
                                expected, actual
                            ));
                        }
                    }
                    _ => return Err("Last entry in history isn't a forward".to_string()),
                }
                Ok(())
            }
        }
    }
}

fn make_chain_selector(
    tip: Point,
    chain_store: &impl ChainStore<Header>,
    peers: &Vec<Peer>,
) -> Arc<Mutex<HeadersTree<Header>>> {
    let root = match tip {
        Origin => None,
        Specific(..) => match chain_store.load_header(&Hash::from(&tip)) {
            None => panic!("Tip {:?} not found in chain store", tip),
            Some(header) => Some(header),
        },
    };

    let mut tree = HeadersTree::new(DEFAULT_MAXIMUM_FRAGMENT_LENGTH, &root);
    let root_hash = root.map(|r| r.hash()).unwrap_or(Origin.hash());
    for peer in peers {
        tree.initialize_peer(peer, &root_hash)
            .expect("the root node is guaranteed to already be in the tree")
    }
    Arc::new(Mutex::new(tree))
}
