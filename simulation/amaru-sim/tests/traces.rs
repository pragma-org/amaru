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

use amaru_consensus::consensus::effects::FetchBlockEffect;
use amaru_consensus::consensus::errors::ConsensusError;
use amaru_sim::simulator::{
    Args, GeneratedEntries, NodeConfig, SimulateConfig, generate_entries, run::spawn_node,
};
use amaru_tracing_json::assert_spans_trees;
use parking_lot::Mutex;
use pure_stage::simulation::SimulationBuilder;
use pure_stage::simulation::running::OverrideResult;
use pure_stage::{Instant, StageGraph};
use rand::SeedableRng;
use rand::prelude::StdRng;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Runtime;
use tracing::info_span;

// FIXME: this test is flaky although it should not as everything is
// supposed to be deterministic in the simulation. Perhaps this
// happens because of the tracing library?
#[test]
#[ignore]
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
    };
    let node_config = NodeConfig::from(args.clone());

    let rt = Runtime::new().unwrap();
    let generate_one = |rng: Arc<Mutex<StdRng>>| {
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
    let rng = Arc::new(Mutex::new(StdRng::seed_from_u64(simulate_config.seed)));
    let generated_entries = generate_one(rng.clone());
    let msg = generated_entries
        .entries()
        .first()
        .unwrap()
        .clone()
        .envelope;

    let execute = || {
        let mut network = SimulationBuilder::default();
        let (input, _, _) = spawn_node("n1".to_string(), node_config.clone(), &mut network, &rt);
        let mut running = network.run(rt.handle().clone());
        running.override_external_effect(usize::MAX, |_eff: Box<FetchBlockEffect>| {
            OverrideResult::Handled(Box::new(Ok::<Vec<u8>, ConsensusError>(vec![])))
        });
        info_span!("handle_msg").in_scope(|| {
            running.enqueue_msg(&input, [msg]);
            _ = running.run_until_blocked();
        });
    };

    assert_spans_trees(
        execute,
        vec![json!(
        {
         "name": "handle_msg",
         "children": [
           {
             "name": "chain_sync.receive_header",
             "children": [
               {
                 "name": "chain_sync.decode_header"
               }
             ]
           },
           {
             "name": "chain_sync.track_peers"
           },
           {
             "name": "chain_sync.validate_header"
           },
           {
             "name": "diffusion.fetch_block"
           },
           {
             "name": "chain_sync.validate_block"
           },
           {
             "name": "chain_sync.select_chain"
           },
           {
             "name": "diffusion.forward_chain"
           }
         ]
        }
         )],
    );
}
