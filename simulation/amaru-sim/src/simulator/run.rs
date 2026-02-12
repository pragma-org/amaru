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

use crate::simulator::simulate::{
    create_symlink_dir, persist_args, persist_generated_actions_as_json, persist_traces,
};
use crate::{
    simulator::{
        Args, History, NodeHandle, RunConfig, generate::generate_entries, simulate::simulate,
    },
    sync::ChainSyncMessage,
};
use amaru::stages::build_node::build_node;
use amaru::tests::configuration::NodeType::{DownstreamNode, NodeUnderTest, UpstreamNode};
use amaru::tests::configuration::{NodeTestConfig, NodeType};
use amaru::tests::in_memory_connection_provider::InMemoryConnectionProvider;
use amaru::tests::setup::{Node, create_node, create_nodes, run_nodes};
use amaru_consensus::headers_tree::data_generation::Action;
use amaru_consensus::{
    effects::{ResourceBlockValidation, ResourceHeaderValidation},
    headers_tree::data_generation::{Chain, GeneratedActions},
    stages::select_chain::DEFAULT_MAXIMUM_FRAGMENT_LENGTH,
};
use amaru_kernel::cardano::network_block::NETWORK_BLOCK;
use amaru_kernel::{
    BlockHeader, GlobalParameters, HeaderHash, IsHeader, Peer, Point, RawBlock, Transaction,
    utils::string::{ListDebug, ListToString, ListsToString},
};
use amaru_ouroboros::{
    ChainStore, ConnectionsResource, Nonces, ReadOnlyChainStore, ResourceMempool, StoreError,
    can_validate_blocks::mock::{MockCanValidateBlocks, MockCanValidateHeaders},
    get_best_chain_block_headers,
    in_memory_consensus_store::InMemConsensusStore,
};
use amaru_protocols::store_effects::ResourceHeaderStore;
use anyhow::anyhow;
use delegate::delegate;
use pure_stage::{
    Instant, StageGraph,
    simulation::{RandStdRng, SimulationBuilder},
    trace_buffer::{TraceBuffer, TraceEntry},
};
use std::fs::create_dir_all;
use std::iter::once;
use std::time::SystemTime;
use std::{sync::Arc, time::Duration};
use tokio::runtime::Runtime;

/// Run the full simulation:
///
/// * Create a simulation environment.
/// * Run the simulation.
pub fn run_test(run_config: &RunConfig) {
    let actions = generate_actions(&run_config);
    let mut rng = run_config.rng();
    let mut nodes = create_nodes(&mut rng, node_configs(&run_config, &actions)).unwrap();
    run_nodes(&mut rng, &mut nodes, 10000);
}

pub fn run_tests(args: Args) -> anyhow::Result<()> {
    let run_config = RunConfig::from(args.clone());
    tracing::info!(?run_config, "simulate.start");
    let tests_dir = run_config.persist_directory.as_path();
    if !tests_dir.exists() {
        create_dir_all(tests_dir)?;
    }

    let now = SystemTime::now();
    let test_run_name = format!("{}", now.duration_since(SystemTime::UNIX_EPOCH)?.as_secs());
    let test_run_dir = tests_dir.join(test_run_name);
    create_dir_all(&test_run_dir)?;
    create_symlink_dir(test_run_dir.as_path(), tests_dir.join("latest").as_path());
    persist_args(test_run_dir.as_path(), &args, args.persist_on_success)?;

    for test_number in 1..=run_config.number_of_tests {
        let test_run_dir_n = test_run_dir.join(format!("test-{}", test_number));
        create_dir_all(&test_run_dir_n)?;
        create_symlink_dir(
            test_run_dir_n.as_path(),
            test_run_dir_n.parent().unwrap().join("latest").as_path(),
        );

        tracing::info!(
            test_number, total=%run_config.number_of_tests,
            "simulate.generate_test_data"
        );

        let actions = generate_actions(&run_config);
        let mut rng = run_config.rng();
        let mut nodes = create_nodes(&mut rng, node_configs(&run_config, &actions)).unwrap();
        run_nodes(&mut rng, &mut nodes, 10000);

        let result = check_chain_property(&nodes, &actions);
        let persist = run_config.persist_on_success || result.is_err();
        persist_generated_actions_as_json(&test_run_dir_n, &actions.actions())?;
        if let Some(node_under_test) = nodes
            .iter()
            .find(|node| node.config.node_type == NodeUnderTest)
        {
            persist_traces(
                test_run_dir_n.as_path(),
                node_under_test.config.trace_buffer.clone(),
                persist,
            )?;
        }
    }

    Ok(())
}

pub fn node_configs(run_config: &RunConfig, actions: &GeneratedActions) -> Vec<NodeTestConfig> {
    let upstream_peers = run_config.upstream_peers();

    let upstream_nodes = upstream_peers
        .iter()
        .map(|peer| {
            NodeTestConfig::default()
                .with_no_upstream_peers()
                .with_listen_address(&peer.name)
                .with_chain_length(run_config.generated_chain_depth)
                .with_actions(get_peer_actions(actions, peer))
                .with_validated_blocks(get_headers(actions, peer))
                .with_node_type(UpstreamNode)
        })
        .collect::<Vec<_>>();

    let listen_address = "127.0.0.1:3000";
    let trace_buffer = Arc::new(parking_lot::Mutex::new(TraceBuffer::new(42, 1_000_000_000)));
    let node_under_test = NodeTestConfig::default()
        .with_listen_address(listen_address)
        .with_chain_length(run_config.generated_chain_depth)
        .with_upstream_peers(upstream_peers)
        .with_trace_buffer(trace_buffer)
        .with_validated_blocks(vec![get_anchor(actions)])
        .with_node_type(NodeUnderTest);

    let downstream_nodes = run_config
        .downstream_peers()
        .iter()
        .map(|peer| {
            NodeTestConfig::default()
                .with_listen_address(&peer.name)
                .with_chain_length(run_config.generated_chain_depth)
                .with_upstream_peer(Peer::new(listen_address))
                .with_validated_blocks(vec![get_anchor(actions)])
                .with_node_type(DownstreamNode)
        })
        .collect::<Vec<_>>();

    upstream_nodes
        .into_iter()
        .chain(once(node_under_test))
        .chain(downstream_nodes)
        .collect()
}

fn get_peer_actions(actions: &GeneratedActions, peer: &Peer) -> Vec<Action> {
    actions
        .actions_per_peer()
        .get(peer)
        .into_iter()
        .flatten()
        .cloned()
        .collect::<Vec<_>>()
}

fn get_headers(actions: &GeneratedActions, peer: &Peer) -> Vec<BlockHeader> {
    get_peer_actions(actions, peer)
        .into_iter()
        .filter_map(|action| match action {
            Action::RollForward { header, .. } => Some(header),
            Action::Rollback { .. } => None,
        })
        .collect::<Vec<_>>()
}

fn get_anchor(actions: &GeneratedActions) -> BlockHeader {
    actions.generated_tree().tree().value.clone()
}

fn generate_actions(run_config: &RunConfig) -> GeneratedActions {
    let rng = run_config.rng();
    generate_entries(
        run_config.generated_chain_depth,
        &run_config.upstream_peers(),
        Instant::at_offset(Duration::from_secs(0)),
        200.0,
    )(rng)
    .generation_context()
    .clone()
}

/// Property: at the end of the simulation, the chain built from the history of messages received
/// downstream must match one of the best chains built directly from messages coming from upstream peers.
///
/// TODO: at some point we should implement a deterministic tie breaker when multiple best chains exist
/// based on the VRF key of the received headers.
fn check_chain_property(
    nodes: &[Node],
    generated_actions: &GeneratedActions,
) -> anyhow::Result<()> {
    for node in nodes {
        if node.config.node_type == NodeType::DownstreamNode {
            tracing::info!(node_id = %node.config.listen_address, "checking chain property for downstream node");
            let store = node.running.resources().get::<ResourceHeaderStore>()?;
            let actual = get_best_chain_block_headers(store.clone());
            tracing::info!(node_id = %node.config.listen_address, blocks_nb = %actual.len(), "retrieved the best dowstream block headers");

            let generated_tree = generated_actions.generated_tree();
            let best_chains = generated_tree.best_chains();

            if !best_chains.contains(&actual) {
                let actions_as_string: String = generated_actions
                    .actions()
                    .iter()
                    .map(|action| action.pretty_print())
                    .collect::<Vec<_>>()
                    .list_to_string(",\n");

                return Err(anyhow!(
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
                ));
            }
        }
    }
    Ok(())
}

/// Run the full simulation:
///
/// * Create a simulation environment.
/// * Run the simulation.
pub fn run_old(args: Args) {
    let rt = Runtime::new().unwrap();
    let trace_buffer = Arc::new(parking_lot::Mutex::new(TraceBuffer::new(42, 1_000_000_000)));
    let connections = Arc::new(InMemoryConnectionProvider::default());
    let node_config = NodeTestConfig::default()
        .with_upstream_peer(Peer::new("peer-1"))
        .with_chain_length(args.generated_chain_depth)
        .with_seed(args.seed())
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
            &node_config.upstream_peers(),
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
    node_config: NodeTestConfig,
    stage_graph: &mut impl StageGraph,
) -> anyhow::Result<()> {
    tracing::info!(node_id, "node.spawn");

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
    tracing::info!(tree_depth=%statistics.tree_depth,
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
    let node_config = NodeTestConfig::default()
        .with_chain_length(args.generated_chain_depth)
        .with_seed(
            args.seed
                .expect("there must be a seed to replay a simulation"),
        );

    let mut stage_graph = SimulationBuilder::default();
    let _ = create_node(&node_config, &mut stage_graph)?;
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
