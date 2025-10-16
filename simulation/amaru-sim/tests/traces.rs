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

use amaru_sim::simulator::run::spawn_node;
use amaru_sim::simulator::simulate::simulate;
use amaru_sim::simulator::{Args, NodeConfig, NodeHandle, SimulateConfig, generate_entries};
use amaru_tracing_json::assert_spans_trees;
use pallas_crypto::hash::Hash;
use pure_stage::simulation::SimulationBuilder;
use pure_stage::{Instant, StageGraph};
use rand::prelude::StdRng;
use serde_json::json;
use std::time::Duration;
use tokio::runtime::Runtime;

#[test]
fn run_simulator_with_traces() {
    let args = Args {
        stake_distribution_file: "tests/data/stake-distribution.json".into(),
        consensus_context_file: "tests/data/consensus-context.json".into(),
        chain_dir: "./chain.db".into(),
        block_tree_file: "tests/data/chain.json".into(),
        start_header: Hash::from([0; 32]),
        number_of_tests: 1,
        number_of_nodes: 1,
        number_of_upstream_peers: 1,
        number_of_downstream_peers: 1,
        disable_shrinking: true,
        seed: None,
        persist_on_success: false,
    };
    let node_config = NodeConfig::from(args.clone());

    let rt = Runtime::new().unwrap();
    let spawn = |node_id: String| {
        let mut network = SimulationBuilder::default();
        let (input, init_messages, output) = spawn_node(node_id, node_config.clone(), &mut network);
        let running = network.run(rt.handle().clone());
        NodeHandle::from_pure_stage(input, init_messages, output, running).unwrap()
    };

    let generate_one = |rng: &mut StdRng| {
        generate_entries(
            &node_config,
            Instant::at_offset(Duration::from_secs(0)),
            200.0,
        )(rng)
        .into_iter()
        .take(1)
        .collect()
    };

    let simulate_config = SimulateConfig::from(args.clone());

    let execute = || {
        simulate(
            &simulate_config,
            spawn,
            generate_one,
            |_, _| Ok(()),
            Default::default(),
            false,
        )
        .unwrap_or_else(|e| panic!("{e}"))
    };

    assert_spans_trees(
        execute,
        vec![json!(
            {
              "name": "handle_msg",
              "children": [
                {
                  "name": "stage.receive_header",
                  "children": [
                    {
                      "name": "consensus.decode_header"
                    }
                  ]
                },
                {
                  "name": "stage.validate_header",
                },
                {
                  "name": "stage.fetch_block"
                },
                {
                  "name": "stage.validate_block"
                },
                {
                  "name": "stage.select_chain",
                  "children": [
                    {
                      "name": "trim_chain"
                    }
                  ]
                },
                {
                  "name": "stage.forward_chain"
                }
              ]
            }
        )],
    );
}
