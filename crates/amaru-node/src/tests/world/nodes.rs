// Copyright 2026 PRAGMA
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

use amaru_consensus::stages::{forge_block::ForgeBlockMsg, test_utils::start_in_era};
use amaru_metrics::Meter;
use amaru_ouroboros::{ConnectionsResource, MempoolMsg};
use amaru_pure_stage::{
    StageGraph, StageRef,
    simulation::{SimulationBuilder, SimulationRunning},
};
use tokio::runtime::Handle;

use crate::{
    build_node,
    tests::{configuration::NodeTestConfig, setup::override_generate_random_seed},
};

/// Build one production-shaped node graph for the world loop.
///
/// Each call uses its own [`SimulationBuilder`] and returns that graph's [`SimulationRunning`].
/// [`build_node`] installs the production stage graph and resources (including header
/// validation). This function then replaces only [`ConnectionsResource`] with the shared
/// world provider.
///
/// Unlike [`crate::tests::setup::create_node`], this path does not:
/// - set `k` to `chain_length` (production `k` from the network parameters is kept)
/// - stub `ValidateHeaderEffect`
/// - add the test-only actions stage
///
/// Chain-store realign follows [`NodeTestConfig::keep_persisted_best_chain`] (off by default).
pub fn build_world_node(
    node_config: &NodeTestConfig,
    connections: ConnectionsResource,
    tokio_handle: &Handle,
) -> anyhow::Result<SimulationRunning> {
    Ok(build_world_node_with_stages(node_config, connections, tokio_handle)?.0)
}

/// Build a production-shaped node and expose the mempool input for deterministic tests.
///
/// The regular world-node constructor deliberately returns only the running graph. Tests that
/// need to model submission or revalidation can use this variant without bypassing the
/// production mempool stage.
pub fn build_world_node_with_mempool_stage(
    node_config: &NodeTestConfig,
    connections: ConnectionsResource,
    tokio_handle: &Handle,
) -> anyhow::Result<(SimulationRunning, StageRef<MempoolMsg>)> {
    let (running, node_stages) = build_world_node_with_stages(node_config, connections, tokio_handle)?;
    Ok((running, node_stages.mempool_stage()))
}

/// Build a production-shaped node and expose the stage inputs needed by forge race scenarios.
///
/// The returned stage references are only test controls. The node still uses the production
/// forge, mempool, ledger, validation, and storage implementations.
pub fn build_world_node_with_forge_stages(
    node_config: &NodeTestConfig,
    connections: ConnectionsResource,
    tokio_handle: &Handle,
) -> anyhow::Result<(SimulationRunning, StageRef<MempoolMsg>, StageRef<ForgeBlockMsg>)> {
    let (running, node_stages) = build_world_node_with_stages(node_config, connections, tokio_handle)?;
    let forge_stage = node_stages.forge_stage().ok_or_else(|| anyhow::anyhow!("forge credentials are missing"))?;
    Ok((running, node_stages.mempool_stage(), forge_stage))
}

fn build_world_node_with_stages(
    node_config: &NodeTestConfig,
    connections: ConnectionsResource,
    tokio_handle: &Handle,
) -> anyhow::Result<(SimulationRunning, crate::stages::build_stage_graph::NodeStages)> {
    let offset = node_config.global_epoch_offset.unwrap_or(start_in_era().relative_time);
    let mut stage_graph = SimulationBuilder::default()
        .with_seed(node_config.seed)
        .with_mailbox_size(node_config.mailbox_size)
        .with_trace_buffer(node_config.trace_buffer.clone())
        .with_global_epoch_offset(offset);

    let random_seed = node_config.generate_random_seed();
    let node_config = node_config.clone().with_connections(connections);
    let config = node_config.make_node_configuration()?;
    let global_parameters = config.global_parameters().clone();

    let node_stages = build_node(&config, &global_parameters, Arc::new(Meter::default()), &mut stage_graph)?;
    let dummy_ledger = node_config.dummy_ledger_dir();
    stage_graph.resources().put::<ConnectionsResource>(node_config.connections);
    if let Some(tmp) = dummy_ledger {
        stage_graph.resources().put(crate::tests::configuration::DummyLedgerDir(tmp));
    }

    let mut running = stage_graph.run(tokio_handle);
    override_generate_random_seed(&mut running, random_seed);
    Ok((running, node_stages))
}
