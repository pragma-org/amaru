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

use amaru::tests::{configuration::NodeTestConfig, setup::create_node};
use amaru_protocols::deserializers::register_deserializers;
use pure_stage::{simulation::SimulationBuilder, trace_buffer::TraceEntry};

use crate::simulator::{Args, RunConfig, generate_actions};

/// Replay a previous run based on a trace
pub fn replay(args: Args, traces: Vec<TraceEntry>) -> anyhow::Result<()> {
    let _guards = register_deserializers();
    let run_config = RunConfig::from(args.clone());
    let actions = generate_actions(&run_config);
    let anchor = actions.get_anchor();

    let node_config = NodeTestConfig::default()
        .with_chain_length(args.generated_chain_depth)
        .with_seed(args.seed.expect("there must be a seed to replay a simulation"))
        .with_upstream_peers(run_config.upstream_peers())
        .with_validated_blocks(vec![anchor]);

    let mut stage_graph = SimulationBuilder::default();
    let _ = create_node(&node_config, &mut stage_graph)?;
    let mut replay = stage_graph.replay();
    replay.run_trace(traces)
}
