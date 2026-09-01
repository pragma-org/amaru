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

use amaru_consensus::stages::test_utils::start_in_era;
use amaru_metrics::Meter;
use amaru_ouroboros::ConnectionsResource;
use amaru_pure_stage::{
    StageGraph,
    simulation::{SimulationBuilder, SimulationRunning},
};
use tokio::runtime::Handle;

use crate::{build_node, tests::configuration::NodeTestConfig};

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
    let offset = node_config.global_epoch_offset.unwrap_or(start_in_era().relative_time);
    let mut stage_graph = SimulationBuilder::default()
        .with_seed(node_config.seed)
        .with_mailbox_size(node_config.mailbox_size)
        .with_trace_buffer(node_config.trace_buffer.clone())
        .with_global_epoch_offset(offset);

    let node_config = node_config.clone().with_connections(connections);
    let config = node_config.make_node_configuration()?;
    let global_parameters = config.global_parameters().clone();

    build_node(&config, &global_parameters, Arc::new(Meter::default()), &mut stage_graph)?;
    let dummy_ledger = node_config.dummy_ledger_dir();
    stage_graph.resources().put::<ConnectionsResource>(node_config.connections);
    if let Some(tmp) = dummy_ledger {
        stage_graph.resources().put(crate::tests::configuration::DummyLedgerDir(tmp));
    }

    Ok(stage_graph.run(tokio_handle))
}
