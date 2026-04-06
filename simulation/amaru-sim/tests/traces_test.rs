#![recursion_limit = "4096"]

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

use amaru::tests::{configuration::NodeTestConfig, setup::create_nodes};
use amaru_consensus::headers_tree::data_generation::Action;
use amaru_kernel::{Peer, any_headers_chain, utils::tests::run_strategy};
use amaru_tracing_json::{TraceCollectConfig, assert_spans_trees};
use pure_stage::simulation::RandStdRng;
use serde_json::json;
use tracing::info_span;

/// For this test we initialize an initiator and a responder node, both set on the first header of a chain of 3 headers.
/// We then execute a RollForward action on the responder.
/// We assert that the resulting trace contains:
///  - Spans for the initialization of the connection, the handshake, the keepalive, the chainsync, and the txsubmission protocols.
///  - Spans for the roll forward action.
#[test]
#[ignore = "This test verifies tracing spans are properly emitted, but it cannot be reliably tested in CI due to reliance on environment configuration."]
fn run_simulator_with_traces() {
    let execute = || {
        let headers = run_strategy(any_headers_chain(3));
        let actions = vec![Action::RollForward { peer: Peer::new("1"), header: headers[1].clone() }];
        let responder_config = NodeTestConfig::responder()
            .with_chain_length(3)
            .with_validated_blocks(headers.clone())
            .with_actions(actions);
        let initiator_config =
            NodeTestConfig::initiator().with_chain_length(3).with_validated_blocks(vec![headers[0].clone()]);

        let mut rng = RandStdRng::from_seed(42);
        info_span!(target: "amaru_consensus", "handle_msg").in_scope(|| {
            let mut nodes = create_nodes(&mut rng, vec![initiator_config, responder_config]).unwrap();
            nodes.run(&mut rng);
        });
    };

    let config = TraceCollectConfig::default()
        .with_include_targets(&["amaru_consensus", "amaru_protocols"])
        .with_exclude_targets(&["amaru_protocols::mux"]);

    assert_spans_trees(
        execute,
        vec![json!(
          {
            "target": "amaru_consensus",
            "name": "handle_msg",
            "children": [
                { "target": "amaru_consensus::stages::fetch_block", "name": "fetch_block" },
                { "target": "amaru_consensus::stages::validate_header", "name": "validate_header" },
                { "target": "amaru_consensus::stages::select_chain", "name": "select_chain" },
                { "target": "amaru_consensus::stages::receive_header", "name": "receive_header" },
                { "target": "amaru_consensus::stages::validate_block", "name": "validate_block" },
                { "target": "amaru_consensus::stages::receive_header", "name": "receive_header" },
                { "target": "amaru_consensus::stages::validate_header", "name": "validate_header" },
                { "target": "amaru_consensus::stages::fetch_block", "name": "fetch_block" },
                { "target": "amaru_consensus::stages::select_chain", "name": "select_chain" },
                { "target": "amaru_consensus::stages::validate_block", "name": "validate_block" },
                { "target": "amaru_consensus::stages::forward_chain", "name": "forward_chain" }
            ]
          }
        )],
        config,
    );
}
