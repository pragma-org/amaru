// Copyright 2024 PRAGMA
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

use crate::stages::build_node::build_node;
use crate::tests::configuration::{NodeTestConfig, NodeType};
use crate::tests::in_memory_connection_provider::InMemoryConnectionProvider;
use amaru_consensus::effects::{ResourceBlockValidation, ResourceHeaderValidation};
use amaru_consensus::headers_tree::data_generation::Action;
use amaru_kernel::{BlockHeight, GlobalParameters, IsHeader, Tip, Transaction};
use amaru_ouroboros_traits::can_validate_blocks::mock::{
    MockCanValidateBlocks, MockCanValidateHeaders,
};
use amaru_ouroboros_traits::{ChainStore, ConnectionsResource, ResourceMempool};
use amaru_protocols::manager::ManagerMessage;
use amaru_protocols::store_effects::Store;
use futures_util::FutureExt;
use pure_stage::simulation::{Blocked, RandStdRng, SimulationBuilder, SimulationRunning};
use pure_stage::{Effects, StageGraph, StageGraphRunning, StageRef};
use std::collections::VecDeque;
use std::fmt::{Debug, Formatter};
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

/// A node with its identifier for logging purposes.
pub struct Node {
    pub node_id: String,
    pub config: NodeTestConfig,
    pub running: SimulationRunning,
    pub manager_stage: StageRef<ManagerMessage>,
    pub actions_stage: StageRef<Action>,
    /// Actions that are pending to be enqueued one at a time for interleaved execution.
    pub pending_actions: VecDeque<Action>,
}

impl Debug for Node {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Node")
            .field("node_id", &self.node_id)
            .field("config", &self.config)
            .finish()
    }
}

impl Node {
    /// Enter a tracing span with this node's identifier for logging purposes.
    pub fn enter_span(&self) -> tracing::span::EnteredSpan {
        tracing::info_span!("node", id = %self.node_id).entered()
    }

    pub fn node_type(&self) -> NodeType {
        self.config.node_type
    }

    pub fn is_node_under_test(&self) -> bool {
        self.config.node_type == NodeType::NodeUnderTest
    }
}

pub fn create_nodes(
    rng: &mut RandStdRng,
    configs: Vec<NodeTestConfig>,
) -> anyhow::Result<Vec<Node>> {
    let connections: ConnectionsResource = Arc::new(InMemoryConnectionProvider::default());
    let mut nodes = vec![];

    for config in configs {
        let _span = config.enter_span();
        let node_id = config.listen_address.clone();

        let mut stage_graph = SimulationBuilder::default()
            .with_seed(config.seed)
            .with_mailbox_size(10000)
            .with_trace_buffer(config.trace_buffer.clone());

        let pending_actions = VecDeque::from(config.actions.clone());
        let config = config.with_connections(connections.clone());
        let (manager_stage, actions_stage) = create_node(&config, &mut stage_graph)?;
        nodes.push(Node {
            node_id,
            config,
            running: stage_graph.run(),
            manager_stage,
            actions_stage,
            pending_actions,
        });
    }

    // We inititialize the nodes by running a few steps to let them run the initialization phase
    // of the miniprotocols
    tracing::info!("Initializing nodes");
    initialize_nodes(rng, &mut nodes, 1000);
    Ok(nodes)
}

pub fn initialize_nodes(rng: &mut RandStdRng, nodes: &mut [Node], max_steps: usize) {
    run_nodes_effect(rng, nodes, max_steps, true);
}

pub fn run_nodes(rng: &mut RandStdRng, nodes: &mut [Node], max_steps: usize) {
    run_nodes_effect(rng, nodes, max_steps, false);
}

/// Run nodes with fine-grained interleaving.
/// Each step runs exactly one effect on a randomly selected node.
pub fn run_nodes_effect(
    rng: &mut RandStdRng,
    nodes: &mut [Node],
    max_steps: usize,
    initialization: bool,
) {
    for step in 0..max_steps {
        for node in nodes.iter_mut() {
            let _span = node.enter_span();
            // Poll external effects (non-blocking)
            node.running.await_external_effect().now_or_never();
            // Wake up stages that have pending messages in their mailboxes
            node.running.receive_inputs();

            // Enqueue one pending action if available (enables proper interleaving)
            if !initialization && let Some(action) = node.pending_actions.pop_front() {
                node.running.enqueue_msg(&node.actions_stage, [action]);
            }
        }

        let Some(node) = pick_random_active_node(rng, nodes) else {
            tracing::info!("All nodes terminated at step {step}");
            return;
        };
        let _span = node.enter_span();

        // Run exactly ONE effect
        match node.running.try_effect() {
            Ok(effect) => {
                node.running.handle_effect(effect);
            }
            Err(Blocked::Sleeping { .. }) => {
                // Advance clock to next wakeup
                node.running.skip_to_next_wakeup(None);
            }
            Err(Blocked::Idle) | Err(Blocked::Busy { .. }) => {
                // Node is blocked, continue to next step (will try another node)
            }
            Err(Blocked::Terminated(_)) => {
                // Node terminated, will be filtered out next iteration
            }
            Err(Blocked::Deadlock(_)) => {
                tracing::warn!("Deadlock detected at step {step}");
            }
            Err(Blocked::Breakpoint(..)) => {
                // Breakpoint hit - could be handled if needed
            }
        }
    }
    tracing::info!("Nodes ran for {max_steps} steps");
}

/// Pick a random non-terminated node from the list.
fn pick_random_active_node<'a>(
    rng: &mut RandStdRng,
    nodes: &'a mut [Node],
) -> Option<&'a mut Node> {
    let active_indices: Vec<usize> = nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| !n.running.is_terminated())
        .map(|(i, _)| i)
        .collect();

    if active_indices.is_empty() {
        return None;
    }

    let idx = active_indices[rng.random_range(0..active_indices.len())];
    Some(&mut nodes[idx])
}

pub fn start_responder(
    simulation_builder: &mut impl StageGraph,
    connections: ConnectionsResource,
) -> anyhow::Result<()> {
    let configuration = NodeTestConfig::responder().with_connections(connections);
    create_node(&configuration, simulation_builder)?;
    Ok(())
}

pub fn start_initiator(
    simulation_builder: &mut SimulationBuilder,
    connections: ConnectionsResource,
) -> anyhow::Result<()> {
    let configuration = NodeTestConfig::initiator().with_connections(connections);
    create_node(&configuration, simulation_builder)?;
    Ok(())
}

/// Create a node and populate its resources from the configuration
pub fn create_node(
    node_config: &NodeTestConfig,
    stage_graph: &mut impl StageGraph,
) -> anyhow::Result<(StageRef<ManagerMessage>, StageRef<Action>)> {
    let config = node_config.make_node_configuration();
    let global_parameters: &GlobalParameters = config.network.into();
    let mut global_parameters = global_parameters.clone();
    global_parameters.consensus_security_param = node_config.chain_length;
    let manager_stage = build_node(&config, &global_parameters, None, stage_graph)?;
    let actions_stage = stage_graph.stage("actions", actions_stage);
    let actions_stage = stage_graph.wire_up(actions_stage, manager_stage.clone());
    set_resources(node_config, stage_graph)?;
    Ok((manager_stage, actions_stage.without_state()))
}

#[expect(clippy::unwrap_used)]
async fn actions_stage(
    manager_stage: StageRef<ManagerMessage>,
    msg: Action,
    eff: Effects<Action>,
) -> StageRef<ManagerMessage> {
    tracing::info!("Received action: {msg:?}");
    let store = Store::new(eff.clone());
    match msg {
        Action::RollForward { header, .. } => {
            tracing::info!(point = %header.point(), "rollforward");
            store.store_header(&header).unwrap();
            store.roll_forward_chain(&header.point()).unwrap();
            store.set_best_chain_hash(&header.hash()).unwrap();
            let tip = Tip::new(header.point(), BlockHeight::from(header.slot().as_u64()));
            eff.send(&manager_stage, ManagerMessage::NewTip(tip)).await;
        }
        Action::Rollback { rollback_point, .. } => {
            tracing::info!(point = %rollback_point, "rollback");
            store.rollback_chain(&rollback_point).unwrap();
            store.set_best_chain_hash(&rollback_point.hash()).unwrap();
            let tip = Tip::new(
                rollback_point,
                BlockHeight::from(rollback_point.slot_or_default().as_u64()),
            );
            eff.send(&manager_stage, ManagerMessage::NewTip(tip)).await;
        }
    }
    manager_stage
}

/// Add resources depending on the simulation configuration.
/// For example this function can be used to set a different chain store for the initiator and the responder.
fn set_resources(
    node_config: &NodeTestConfig,
    stage_graph: &mut impl StageGraph,
) -> anyhow::Result<()> {
    stage_graph
        .resources()
        .put::<ResourceBlockValidation>(Arc::new(MockCanValidateBlocks));
    stage_graph
        .resources()
        .put::<ResourceHeaderValidation>(Arc::new(MockCanValidateHeaders));
    stage_graph
        .resources()
        .put::<ResourceMempool<Transaction>>(node_config.mempool.clone());
    stage_graph.resources().put(node_config.connections.clone());
    Ok(())
}

/// Set up logging to the console (enable logs with the RUST_LOG env var, for example RUST_LOG=info)
pub fn setup_logging(enable: bool) {
    if !enable {
        return;
    };
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_test_writer()
        .try_init();
}
