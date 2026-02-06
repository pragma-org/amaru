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

use amaru_observability::trace_span;
use amaru_sim::simulator::{
    Args, GeneratedEntries, NodeConfig, SimulateConfig, TEST_DATA_DIR, generate_entries,
    run::spawn_node,
};
use amaru_tracing_json::assert_spans_trees;
use pure_stage::{
    Instant,
    simulation::{RandStdRng, SimulationBuilder},
};
use rand::{SeedableRng, prelude::StdRng};
use serde_json::json;
use std::time::Duration;
use tokio::runtime::Runtime;

// FIXME: this test is flaky although it should not as everything is
// supposed to be deterministic in the simulation. Perhaps this
// happens because of the tracing library?
#[test]
fn run_simulator_with_traces() {
    let args = Args {
        number_of_tests: 1,
        number_of_nodes: 1,
        number_of_upstream_peers: 1,
        number_of_downstream_peers: 1,
        generated_chain_depth: 1,
        disable_shrinking: true,
        seed: Some(43),
        persist_on_success: false,
        persist_directory: format!("{TEST_DATA_DIR}/run_simulator_with_traces"),
    };
    let node_config = NodeConfig::from(args.clone());

    let rt = Runtime::new().unwrap();
    let generate_one = |rng: RandStdRng| {
        let generated_entries = generate_entries(
            &node_config,
            Instant::at_offset(Duration::from_secs(0)),
            200.0,
        )(rng);

        let generation_context = generated_entries.generation_context().clone();
        GeneratedEntries::new(
            generated_entries
                .entries()
                .iter()
                .take(1)
                .cloned()
                .collect(),
            generation_context,
        )
    };

    let simulate_config = SimulateConfig::from(args.clone());
    let mut rng = RandStdRng(StdRng::seed_from_u64(simulate_config.seed));
    let generated_entries = generate_one(rng.derive());
    let msg = generated_entries
        .entries()
        .first()
        .unwrap()
        .clone()
        .envelope;

    let execute = || {
        let mut network = SimulationBuilder::default();
        let (input, _, _) = spawn_node("n1".to_string(), node_config.clone(), &mut network);
        let mut running = network.run();
        trace_span!(simulator::node::HANDLE_MSG).in_scope(|| {
            running.enqueue_msg(&input, [msg]);
            running
                .run_until_blocked_incl_effects(rt.handle())
                .assert_terminated("processing_errors");
        });
    };

    assert_spans_trees(
        execute,
        vec![json!(
          {
            "name": "simulator::node::HANDLE_MSG",
            "children": [
              {
                "name": "consensus::chain_sync::receive_header",
                "children": [
                  {
                    "name": "decode_header"
                  }
                ]
              },
              {
                "name": "consensus::chain_sync::validate_header"
              },
              {
                "name": "consensus::diffusion::fetch_block"
              },
              {
                "name": "consensus::chain_sync::validate_block"
              },
              {
                "name": "consensus::chain_sync::select_chain"
              },
              {
                "name": "consensus::diffusion::forward_chain"
              }
            ]
          }
        )],
    );
}
