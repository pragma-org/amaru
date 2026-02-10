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

use crate::{
    simulator::{
        Args, History, NodeHandle, RunConfig, generate::generate_entries, simulate::simulate,
    },
    sync::ChainSyncMessage,
};
use amaru::stages::build_node::build_node;
use amaru::tests::configuration::NodeConfig;
use amaru::tests::in_memory_connection_provider::InMemoryConnectionProvider;
use amaru_consensus::{
    effects::{ResourceBlockValidation, ResourceHeaderValidation},
    headers_tree::data_generation::{Chain, GeneratedActions},
    stages::select_chain::DEFAULT_MAXIMUM_FRAGMENT_LENGTH,
};
use amaru_kernel::cardano::network_block::NETWORK_BLOCK;
use amaru_kernel::{
    BlockHeader, GlobalParameters, HeaderHash, IsHeader, Point, RawBlock, Transaction,
    utils::string::{ListDebug, ListToString, ListsToString},
};
use amaru_ouroboros::{
    ChainStore, ConnectionsResource, Nonces, ReadOnlyChainStore, ResourceMempool, StoreError,
    can_validate_blocks::mock::{MockCanValidateBlocks, MockCanValidateHeaders},
    in_memory_consensus_store::InMemConsensusStore,
};
use amaru_protocols::store_effects::ResourceHeaderStore;
use delegate::delegate;
use pure_stage::{
    Instant, StageGraph,
    simulation::{RandStdRng, SimulationBuilder},
    trace_buffer::{TraceBuffer, TraceEntry},
};
use rand::SeedableRng;
use rand::prelude::StdRng;
use std::{sync::Arc, time::Duration};
use tokio::runtime::Runtime;
use tracing::info;

/// Run the full simulation:
///
/// * Create a simulation environment.
/// * Run the simulation.
pub fn run(args: Args) {
    let rt = Runtime::new().unwrap();
    let rng = RandStdRng(StdRng::seed_from_u64(args.seed()));
    let trace_buffer = Arc::new(parking_lot::Mutex::new(TraceBuffer::new(42, 1_000_000_000)));
    let connections = Arc::new(InMemoryConnectionProvider::default());
    let node_config = NodeConfig::default()
        .with_upstream_peer("peer-1")
        .with_chain_length(args.generated_chain_depth as usize)
        .with_eval_strategy(rng)
        .with_trace_buffer(trace_buffer.clone())
        .with_connections(connections);

    let spawn = |node_id: String, rng: RandStdRng| {
        let mut network = SimulationBuilder::default()
            .with_trace_buffer(trace_buffer.clone())
            .with_mailbox_size(10000)
            .with_eval_strategy(rng);
        spawn_node(node_id, node_config.clone(), &mut network).unwrap();
        let running = network.run();
        NodeHandle::from_pure_stage(running, rt.handle().clone()).unwrap()
    };

    let run_config = RunConfig::from(args.clone());
    simulate(
        &run_config,
        &node_config,
        spawn,
        generate_entries(
            node_config.chain_length,
            node_config.upstream_peers.len(),
            Instant::at_offset(Duration::from_secs(0)),
            200.0,
        ),
        chain_property(),
        display_entries_statistics,
        trace_buffer.clone(),
        args.persist_on_success,
    )
    .unwrap_or_else(|e| panic!("{e}"));
}

/// Create and start a node
pub fn spawn_node(
    node_id: String,
    node_config: NodeConfig,
    stage_graph: &mut impl StageGraph,
) -> anyhow::Result<()> {
    info!(node_id, "node.spawn");

    let config = node_config.make_node_configuration();
    let global_parameters: &GlobalParameters = config.network.into();
    let mut global_parameters = global_parameters.clone();

    // Set the maximum length for the best fragment based on the generated chain depth
    // so that the generate roll forwards will move the best chain anchor.
    global_parameters.consensus_security_param =
        if node_config.chain_length > DEFAULT_MAXIMUM_FRAGMENT_LENGTH {
            DEFAULT_MAXIMUM_FRAGMENT_LENGTH
        } else {
            node_config.chain_length - 1
        };

    build_node(&config, &global_parameters, None, stage_graph)?;

    // Override some resources to use mocks
    stage_graph
        .resources()
        .put::<ResourceMempool<Transaction>>(node_config.mempool.clone());
    stage_graph
        .resources()
        .put::<ConnectionsResource>(node_config.connections.clone());
    stage_graph
        .resources()
        .put::<ResourceHeaderValidation>(Arc::new(MockCanValidateHeaders));
    stage_graph
        .resources()
        .put::<ResourceBlockValidation>(Arc::new(MockCanValidateBlocks));

    Ok(())
}

/// Property: at the end of the simulation, the chain built from the history of messages received
/// downstream must match one of the best chains built directly from messages coming from upstream peers.
///
/// TODO: at some point we should implement a deterministic tie breaker when multiple best chains exist
/// based on the VRF key of the received headers.
fn chain_property() -> impl Fn(&History<ChainSyncMessage>, &GeneratedActions) -> Result<(), String>
{
    move |history, generated_actions| {
        let actual = make_best_chain_from_downstream_messages(history)?;
        let generated_tree = generated_actions.generated_tree();
        let best_chains = generated_tree.best_chains();

        if !best_chains.contains(&actual) {
            let actions_as_string: String = generated_actions
                .actions()
                .iter()
                .map(|action| action.pretty_print())
                .collect::<Vec<_>>()
                .list_to_string(",\n");

            Err(format!(
                r#"
The actual chain

{}

is not in the best chains

{}

The headers tree is
{}

The actions are

{}
"#,
                actual.list_to_string(",\n  "),
                best_chains.lists_to_string(",\n  ", ",\n  "),
                generated_actions.generated_tree().tree(),
                actions_as_string
            ))
        } else {
            Ok(())
        }
    }
}

/// Generate statistics from actions and log them.
fn display_entries_statistics(generated_actions: &GeneratedActions) {
    let statistics = generated_actions.statistics();
    info!(tree_depth=%statistics.tree_depth,
          tree_nodes=%statistics.number_of_nodes,
          tree_forks=%statistics.number_of_fork_nodes,
          "simulate.generate_test_data.statistics");
}

/// Build the best chain from messages sent to downstream peers.
fn make_best_chain_from_downstream_messages(
    history: &History<ChainSyncMessage>,
) -> Result<Chain, String> {
    let mut best_chain = vec![];
    for (i, message) in history.0.iter().enumerate() {
        // only consider messages sent to the first peer
        if !message.dest.starts_with("c1") {
            continue;
        };
        match &message.body {
            msg @ ChainSyncMessage::Fwd { .. } => {
                if let Some(header) = msg.decode_block_header() {
                    best_chain.push(header)
                }
            }
            msg @ ChainSyncMessage::Bck { .. } => {
                if let Some(header_hash) = msg.header_hash() {
                    let rollback_position = best_chain.iter().position(|h| h.hash() == header_hash);
                    if let Some(rollback_position) = rollback_position {
                        best_chain.truncate(rollback_position + 1);
                    } else {
                        Err(format!(
                            "after the action {}, we have a rollback position that does not exist with hash {}.\nThe best chain is:\n{}. The history is:\n{}",
                            i + 1,
                            header_hash,
                            best_chain.list_to_string(",\n"),
                            history.0.iter().collect::<Vec<_>>().list_debug("\n")
                        ))?;
                    }
                }
            }
            _ => (),
        }
    }
    Ok(best_chain)
}

/// Replay a previous simulation run:
pub fn replay(args: Args, traces: Vec<TraceEntry>) -> anyhow::Result<()> {
    let node_config = NodeConfig::default()
        .with_chain_length(args.generated_chain_depth as usize)
        .with_seed(
            args.seed
                .expect("there must be a seed to replay a simulation"),
        );
    let mut stage_graph = SimulationBuilder::default();
    let _ = spawn_node("n1".to_string(), node_config, &mut stage_graph);
    stage_graph
        .resources()
        .put::<ResourceHeaderStore>(Arc::new(ReplayStore::default()));
    let mut replay = stage_graph.replay();
    replay.run_trace(traces)
}

#[derive(Clone, Default)]
struct ReplayStore {
    inner: InMemConsensusStore<BlockHeader>,
}

impl ReadOnlyChainStore<BlockHeader> for ReplayStore {
    delegate! {
        to self.inner {
            fn load_header(&self, hash: &HeaderHash) -> Option<BlockHeader>;
            fn get_children(&self, hash: &HeaderHash) -> Vec<HeaderHash>;
            fn get_anchor_hash(&self) -> HeaderHash;
            fn get_best_chain_hash(&self) -> HeaderHash;
            fn load_from_best_chain(&self, point: &Point) -> Option<HeaderHash>;
            fn next_best_chain(&self, point: &Point) -> Option<Point>;
            fn get_nonces(&self, header: &HeaderHash) -> Option<Nonces>;
            fn has_header(&self, hash: &HeaderHash) -> bool;
        }
    }

    fn load_block(&self, _hash: &HeaderHash) -> Result<Option<RawBlock>, StoreError> {
        Ok(Some(NETWORK_BLOCK.raw_block()))
    }
}

impl ChainStore<BlockHeader> for ReplayStore {
    delegate! {
        to self.inner {
            fn store_header(&self, header: &BlockHeader) -> Result<(), StoreError>;
            fn set_anchor_hash(&self, hash: &HeaderHash) -> Result<(), StoreError>;
            fn set_best_chain_hash(&self, hash: &HeaderHash) -> Result<(), StoreError>;
            fn store_block(&self, hash: &HeaderHash, block: &RawBlock) -> Result<(), StoreError>;
            fn put_nonces(&self, header: &HeaderHash, nonces: &Nonces) -> Result<(), StoreError>;
            fn roll_forward_chain(&self, point: &Point) -> Result<(), StoreError>;
            fn rollback_chain(&self, point: &Point) -> Result<usize, StoreError>;
        }
    }
}
