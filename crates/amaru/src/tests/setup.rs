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

use std::sync::Arc;

use amaru_consensus::{
    effects::{ResourceBlockValidation, ResourceHeaderValidation, ResourceTxValidation},
    headers_tree::data_generation::Action,
};
use amaru_kernel::{BlockHeight, GlobalParameters, IsHeader, Tip, Transaction};
use amaru_ouroboros::{
    ChainStore, ConnectionsResource, MockCanValidateTxs, ResourceMempool,
    can_validate_blocks::mock::{MockCanValidateBlocks, MockCanValidateHeaders},
};
use amaru_protocols::{manager::ManagerMessage, store_effects::Store};
use anyhow::anyhow;
use pure_stage::{
    Effects, StageGraph, StageRef, TryInStage,
    simulation::{RandStdRng, SimulationBuilder},
};
use tracing_subscriber::EnvFilter;

use crate::{
    stages::{build_node::build_node, build_stage_graph::NodeStages},
    tests::{
        configuration::NodeTestConfig, in_memory_connection_provider::InMemoryConnectionProvider, node::Node,
        nodes::Nodes,
    },
};

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
        let test_node_stages = create_node(&config, &mut stage_graph)?;
        nodes.push(Node::new(config, stage_graph.run(), test_node_stages));
    }

    // Initialize the nodes by running until the chainsync protocol is registered
    tracing::info!("Initializing nodes");
    let mut nodes = Nodes::new(nodes);
    nodes.initialize(rng);
    Ok(nodes)
}

/// Create a single node according to its configuration
/// and populate its resources.
pub fn create_node(node_config: &NodeTestConfig, stage_graph: &mut impl StageGraph) -> anyhow::Result<TestNodeStages> {
    let config = node_config.make_node_configuration()?;
    let global_parameters: &GlobalParameters = config.network.into();
    let mut global_parameters = global_parameters.clone();

    // The chain length used when generating data is set as the `k` parameter for the node
    // in order to simulate what happens when new tips are added and trigger a move of the best
    // chain anchor.
    global_parameters.consensus_security_param = node_config.chain_length;
    let node_stages = build_node(&config, &global_parameters, None, stage_graph)
        .map_err(|e| anyhow!("Cannot build node.\nThe node config is\n{:?}\n\nThe error is {e:?}", node_config))?;

    // The actions stage allows us to send NewTip messages to the manager so that chainsync
    // events can be sent to the node under test.
    let actions_stage = stage_graph.stage("actions", actions_stage);
    let actions_stage = stage_graph.wire_up(actions_stage, (node_stages.manager_stage.clone(), node_config.seed));

    set_resources(node_config, stage_graph)?;
    Ok(TestNodeStages::new(node_stages, actions_stage.without_state()))
}

/// This data type encapsulates the stages exposed by the processing graph in production
/// + additional stage references to stages added to support testing.
pub struct TestNodeStages {
    node_stages: NodeStages,
    actions_stage: StageRef<Action>,
}

impl TestNodeStages {
    pub fn new(node_stages: NodeStages, actions_stage: StageRef<Action>) -> Self {
        Self { node_stages, actions_stage }
    }

    pub fn manager_stage(&self) -> &StageRef<ManagerMessage> {
        &self.node_stages.manager_stage
    }

    pub fn actions_stage(&self) -> &StageRef<Action> {
        &self.actions_stage
    }
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

type ActionsState = (StageRef<ManagerMessage>, u64);

/// Create an "actions" stage to send NewTip messages to the Manager, and eventually to the node
/// under test.
///
/// In addition to sending the NewTip message, this stage makes sure that the chainstore points
/// to the same tip. This way, when the chainsync miniprotocol executes, it grabs consistent headers
/// from the ChainStore.
///
async fn actions_stage(state: ActionsState, msg: Action, eff: Effects<Action>) -> ActionsState {
    let (manager_stage, seed) = &state;
    tracing::info!("Received action: {msg:?}");
    let store = Store::new(eff.clone());
    let tip = match &msg {
        Action::RollForward { header, .. } => {
            tracing::info!(point = %header.point(), "rollforward");
            store
                .store_header(header)
                .or_terminate(&eff, |e| async move {
                    tracing::error!("Cannot store the header {}: {e:?}. The seed is {seed}", &header);
                })
                .await;
            store
                .roll_forward_chain(&header.point())
                .or_terminate(&eff, |e| async move {
                    tracing::error!("Cannot rollforward chain: {e:?}. The seed is {seed}");
                })
                .await;
            Tip::new(header.point(), BlockHeight::from(header.slot().as_u64()))
        }
        Action::Rollback { rollback_point, .. } => {
            tracing::info!(point = %rollback_point, "rollback");
            store
                .rollback_chain(rollback_point)
                .or_terminate(&eff, |e| async move {
                    tracing::error!("Cannot rollback the chain to {}: {e:?}. The seed is {seed}", &rollback_point,);
                })
                .await;
            Tip::new(*rollback_point, BlockHeight::from(rollback_point.slot_or_default().as_u64()))
        }
    };
    store
        .set_best_chain_hash(&msg.hash())
        .or_terminate(&eff, |e| async move {
            tracing::error!("Cannot set the best chain: {e:?}. The seed is {seed}");
        })
        .await;
    eff.send(manager_stage, ManagerMessage::NewTip(tip)).await;
    state
}

/// Add resources depending on the simulation configuration.
/// For example this function can be used to set a different chain store for the initiator and the responder.
fn set_resources(node_config: &NodeTestConfig, stage_graph: &mut impl StageGraph) -> anyhow::Result<()> {
    stage_graph.resources().put::<ResourceBlockValidation>(Arc::new(MockCanValidateBlocks));
    stage_graph.resources().put::<ResourceHeaderValidation>(Arc::new(MockCanValidateHeaders));
    stage_graph.resources().put::<ResourceTxValidation>(Arc::new(MockCanValidateTxs));
    stage_graph.resources().put::<ResourceMempool<Transaction>>(node_config.mempool.clone());
    stage_graph.resources().put(node_config.connections.clone());
    Ok(())
}

/// Set up logging to the console (enable logs with the RUST_LOG env var, for example RUST_LOG=info)
pub fn setup_logging(enable: bool) {
    if !enable {
        return;
    };
    let _ = tracing_subscriber::fmt().with_env_filter(EnvFilter::from_default_env()).with_test_writer().try_init();
}
