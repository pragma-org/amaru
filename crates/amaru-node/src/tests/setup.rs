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
    effects::{
        ResourceBlockValidation, ResourceConsensusParameters, ResourceEraHistory, ResourceHasStakePools,
        ResourcePoolSummaries, ResourceTxValidation, ValidateHeaderEffect,
    },
    stages::test_utils::start_in_era,
};
use amaru_kernel::{ConsensusParameters, IsHeader, NetworkName, NonEmptyVec, Transaction};
use amaru_metrics::Meter;
use amaru_ouroboros::{
    BaseReadChainStore, ConnectionsResource, DiagnosticChainStore, MockBlockValidator, MockCanValidateTxs, Nonces,
    PoolSummaries, ResourceMempool, has_stake_pools::MockHasStakePools,
};
use amaru_protocols::{
    manager::ManagerMessage,
    store_effects::{ResourceHeaderStore, Store},
};
use amaru_pure_stage::{
    Effects, OrTerminateWith, StageGraph, StageRef,
    simulation::{RandStdRng, SimulationBuilder, running::OverrideResult},
};
use anyhow::anyhow;
use tracing_subscriber::EnvFilter;

use crate::{
    stages::{build_node::build_node, build_stage_graph::NodeStages},
    tests::{
        Action, configuration::NodeTestConfig, in_memory_connection_provider::InMemoryConnectionProvider, node::Node,
        nodes::Nodes,
    },
};

/// Create simulated nodes based on a list of configurations.
/// The random generator is used to generate the test data that is injected into upstream nodes.
///
pub fn create_nodes(
    rng: &mut RandStdRng,
    configs: Vec<NodeTestConfig>,
    tokio_handle: &tokio::runtime::Handle,
) -> anyhow::Result<Nodes> {
    let connections: ConnectionsResource = Arc::new(InMemoryConnectionProvider::default());
    let mut nodes = vec![];

    for config in configs {
        let _span = config.enter_span();

        let mut stage_graph = SimulationBuilder::default()
            .with_seed(config.seed)
            .with_mailbox_size(10000)
            .with_trace_buffer(config.trace_buffer.clone())
            .with_global_epoch_offset(start_in_era().relative_time);

        let config = config.with_connections(connections.clone());
        let test_node_stages = create_node(&config, &mut stage_graph)?;

        let mut running = stage_graph.run(tokio_handle);
        // Don't validate the generated headers, we just want to check the mini-protocols communication.
        running.override_external_effect::<ValidateHeaderEffect>(usize::MAX, |_| {
            OverrideResult::handled(Ok(Nonces::for_tests()))
        });

        nodes.push(Node::new(config, running, test_node_stages));
    }

    // Initialize the nodes by running until the chainsync protocol is registered
    tracing::info!("Initializing nodes");
    let mut nodes = Nodes::new(nodes);
    nodes.initialize(rng);
    Ok(nodes)
}

/// Create a single node according to its configuration
/// and populate its resources.
#[allow(clippy::panic)]
pub fn create_node(node_config: &NodeTestConfig, stage_graph: &mut impl StageGraph) -> anyhow::Result<TestNodeStages> {
    let config = node_config.make_node_configuration()?;
    let mut global_parameters = config.global_parameters().clone();

    // The chain length used when generating data is set as the `k` parameter for the node
    // in order to simulate what happens when new tips are added and trigger a move of the best
    // chain anchor.
    global_parameters.consensus_security_param = node_config.chain_length as u64;
    let node_stages = build_node(&config, &global_parameters, Arc::new(Meter::default()), stage_graph)
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
                .store_validated_header(header, &Nonces::for_tests())
                .or_terminate_with(&eff, |e| async move {
                    tracing::error!("Cannot store the header {}: {e:?}. The seed is {seed}", &header);
                })
                .await;
            store
                .roll_forward_chain(&header.point())
                .or_terminate_with(&eff, |e| async move {
                    tracing::error!("Cannot rollforward chain: {e:?}. The seed is {seed}");
                })
                .await;
            header.point()
        }
        Action::Rollback { rollback_point, .. } => {
            tracing::info!(point = %rollback_point, "rollback");
            let Some(rollback) = store.load_header(&rollback_point.hash()).await.map(|h| h.point()) else {
                tracing::error!(
                    "Cannot rollback the chain to {}: header not in store. The seed is {seed}",
                    rollback_point
                );
                return state;
            };
            store
                .switch_to_fork(&rollback, &NonEmptyVec::singleton(rollback))
                .or_terminate_with(&eff, |e| async move {
                    tracing::error!("Cannot rollback the chain to {}: {e:?}. The seed is {seed}", &rollback_point,);
                })
                .await;
            rollback
        }
    };
    store
        .set_best_chain_tip(&tip)
        .or_terminate_with(&eff, |e| async move {
            tracing::error!("Cannot set the best chain: {e:?}. The seed is {seed}");
        })
        .await;
    eff.send(manager_stage, ManagerMessage::new_tip(tip)).await;
    state
}

/// Add resources depending on the simulation configuration.
/// For example this function can be used to set a different chain store for the initiator and the responder.
fn set_resources(node_config: &NodeTestConfig, stage_graph: &mut impl StageGraph) -> anyhow::Result<()> {
    let block_validation = Arc::new(MockBlockValidator::new(node_config.chain_store.get_best_chain_tip()));
    stage_graph.resources().put::<ResourceHeaderStore>(node_config.chain_store.clone());
    stage_graph.resources().put::<Arc<dyn DiagnosticChainStore>>(node_config.chain_store.clone());
    stage_graph.resources().put::<ResourceBlockValidation>(block_validation.clone());
    stage_graph.resources().put::<ResourceHasStakePools>(Arc::new(MockHasStakePools));
    stage_graph.resources().put::<ResourceTxValidation>(Arc::new(MockCanValidateTxs));

    #[expect(clippy::unwrap_used)]
    let era = NetworkName::Preprod.as_era_history().unwrap();
    #[expect(clippy::expect_used)]
    let global = NetworkName::Preprod.as_global_parameters().cloned().expect("global parameters for preprod");
    let cp = Arc::new(ConsensusParameters::new(global, era));
    stage_graph.resources().put::<ResourceConsensusParameters>(cp);
    stage_graph.resources().put::<ResourceEraHistory>(era.clone());
    stage_graph.resources().put::<ResourcePoolSummaries>(Arc::new(PoolSummaries::default()));
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
