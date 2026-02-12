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
use crate::tests::configuration::NodeTestConfig;
use crate::tests::in_memory_connection_provider::InMemoryConnectionProvider;
use crate::tests::node::Node;
use crate::tests::nodes::Nodes;
use amaru_consensus::effects::{ResourceBlockValidation, ResourceHeaderValidation};
use amaru_consensus::headers_tree::data_generation::Action;
use amaru_kernel::{BlockHeight, GlobalParameters, IsHeader, Tip, Transaction};
use amaru_ouroboros::can_validate_blocks::mock::{MockCanValidateBlocks, MockCanValidateHeaders};
use amaru_ouroboros::{ChainStore, ConnectionsResource, ResourceMempool};
use amaru_protocols::manager::ManagerMessage;
use amaru_protocols::store_effects::Store;
use pure_stage::simulation::{RandStdRng, SimulationBuilder};
use pure_stage::{Effects, StageGraph, StageRef};
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

/// Create simulated nodes based on a list of configurations.
/// The random generator is used to generate the test data that is injected into upstream nodes.
///
pub fn create_nodes(rng: &mut RandStdRng, configs: Vec<NodeTestConfig>) -> anyhow::Result<Nodes> {
    let connections: ConnectionsResource = Arc::new(InMemoryConnectionProvider::default());
    let mut nodes = vec![];

    for config in configs {
        let _span = config.enter_span();

        let mut stage_graph = SimulationBuilder::default()
            .with_seed(config.seed)
            .with_mailbox_size(10000)
            .with_trace_buffer(config.trace_buffer.clone());

        let config = config.with_connections(connections.clone());
        let (manager_stage, actions_stage) = create_node(&config, &mut stage_graph)?;
        nodes.push(Node::new(
            config,
            stage_graph.run(),
            manager_stage,
            actions_stage,
        ));
    }

    // Initialize the nodes by running until the chainsync protocol is registered
    tracing::info!("Initializing nodes");
    let mut nodes = Nodes::new(nodes);
    nodes.initialize(rng);
    Ok(nodes)
}

/// Create a single node according to its configuration
/// and populate its resources.
pub fn create_node(
    node_config: &NodeTestConfig,
    stage_graph: &mut impl StageGraph,
) -> anyhow::Result<(StageRef<ManagerMessage>, StageRef<Action>)> {
    let config = node_config.make_node_configuration();
    let global_parameters: &GlobalParameters = config.network.into();
    let mut global_parameters = global_parameters.clone();

    // The chain length used when generating data is set as the `k` parameter for the node
    // in order to simulate what happens when new tips are added and trigger a move of the best
    // chain anchor.
    global_parameters.consensus_security_param = node_config.chain_length;
    let manager_stage = build_node(&config, &global_parameters, None, stage_graph)?;

    // The actions stage allows us to send NewTip messages to the manager so that chainsync
    // events can be sent to the node under test.
    let actions_stage = stage_graph.stage("actions", actions_stage);
    let actions_stage = stage_graph.wire_up(actions_stage, manager_stage.clone());

    set_resources(node_config, stage_graph)?;
    Ok((manager_stage, actions_stage.without_state()))
}

/// This starts a responder node with a preset configuration for tests.
pub fn start_responder(
    simulation_builder: &mut impl StageGraph,
    connections: ConnectionsResource,
) -> anyhow::Result<()> {
    let configuration = NodeTestConfig::responder().with_connections(connections);
    create_node(&configuration, simulation_builder)?;
    Ok(())
}

/// This starts an initiator node with a preset configuration for tests.
pub fn start_initiator(
    simulation_builder: &mut SimulationBuilder,
    connections: ConnectionsResource,
) -> anyhow::Result<()> {
    let configuration = NodeTestConfig::initiator().with_connections(connections);
    create_node(&configuration, simulation_builder)?;
    Ok(())
}

/// Create an "actions" stage to send NewTip messages to the Manager, and eventually to the node
/// under test.
///
/// In addition to sending the NewTip message, this stage makes sure that the chainstore points
/// to the same tip. This way, when the chainsync miniprotocol executes, it grabs consistent headers
/// from the ChainStore.
///
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
