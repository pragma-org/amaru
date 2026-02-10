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
use crate::tests::configuration::NodeConfig;
use crate::tests::in_memory_connection_provider::InMemoryConnectionProvider;
use amaru_consensus::effects::{ResourceBlockValidation, ResourceHeaderValidation};
use amaru_kernel::Transaction;
use amaru_ouroboros_traits::can_validate_blocks::mock::{
    MockCanValidateBlocks, MockCanValidateHeaders,
};
use amaru_ouroboros_traits::{ConnectionsResource, ResourceMempool};
use futures_util::FutureExt;
use pure_stage::simulation::{SimulationBuilder, SimulationRunning};
use pure_stage::{StageGraph, StageGraphRunning};
use std::sync::Arc;
use tracing::info;
use tracing_subscriber::EnvFilter;

pub fn create_nodes(configs: Vec<NodeConfig>) -> anyhow::Result<Vec<SimulationRunning>> {
    let connections: ConnectionsResource = Arc::new(InMemoryConnectionProvider::default());
    let mut result = vec![];

    for config in configs {
        let mut stage_graph = SimulationBuilder::default();
        let configuration = config.with_connections(connections.clone());
        create_node(&configuration, &mut stage_graph)?;
        result.push(stage_graph.run())
    }
    Ok(result)
}

pub fn run_nodes(nodes: &mut [SimulationRunning]) {
    // We bound the number of steps to prevent an infinite loop in case, for example, of a deadlock.
    for step in 0..10000 {
        // Run each node until it blocks or is terminated
        for node in nodes.iter_mut() {
            node.run_or_terminated();
        }

        // Check if all nodes have terminated
        if nodes.iter().all(|n| n.is_terminated()) {
            info!("Both nodes terminated at step {step}");
            return;
        }

        // Advance external effects for all nodes
        for node in nodes.iter_mut() {
            node.await_external_effect().now_or_never();
        }
    }
    info!("Nodes were terminated after 10000 steps");
}

pub fn start_responder(
    simulation_builder: &mut impl StageGraph,
    connections: ConnectionsResource,
) -> anyhow::Result<()> {
    let configuration = NodeConfig::responder().with_connections(connections);
    create_node(&configuration, simulation_builder)?;
    Ok(())
}

pub fn start_initiator(
    simulation_builder: &mut SimulationBuilder,
    connections: ConnectionsResource,
) -> anyhow::Result<()> {
    let configuration = NodeConfig::initiator().with_connections(connections);
    create_node(&configuration, simulation_builder)?;
    Ok(())
}

/// Create a node and populate its resources from the configuration
pub fn create_node(node_config: &NodeConfig, network: &mut impl StageGraph) -> anyhow::Result<()> {
    let config = node_config.make_node_configuration();
    build_node(&config, config.network.into(), None, network)?;
    set_resources(node_config, network)
}

/// Add resources depending on the simulation configuration.
/// For example this function can be used to set a different chain store for the initiator and the responder.
fn set_resources(
    node_config: &NodeConfig,
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
