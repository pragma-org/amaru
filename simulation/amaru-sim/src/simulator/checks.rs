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

use crate::simulator::test_result::TestResult;
use amaru::tests::nodes::Nodes;
use amaru_consensus::headers_tree::data_generation::GeneratedActions;
use amaru_kernel::utils::string::{ListToString, ListsToString};
use amaru_ouroboros::get_best_chain_block_headers;
use amaru_protocols::store_effects::ResourceHeaderStore;
use anyhow::anyhow;

/// Property: at the end of the simulation, the chain built from the history of messages received
/// downstream must match one of the best chains built directly from messages coming from upstream peers.
///
/// TODO: at some point we should implement a deterministic tie breaker when multiple best chains exist
/// based on the VRF key of the received headers.
pub fn check_chain_property(nodes: Nodes, actions: &GeneratedActions) -> TestResult {
    for node in &nodes {
        if node.is_downstream() {
            tracing::info!(node_id = %node.node_id(), "checking chain property for downstream node");

            let store = node
                .resources()
                .get::<ResourceHeaderStore>()
                .expect("A ResourceHeaderStore must be available")
                .clone();
            let actual = get_best_chain_block_headers(store.clone());
            tracing::info!(node_id = %node.node_id(), blocks_nb = %actual.len(), "retrieved the best downstream block headers");

            let generated_tree = actions.generated_tree();
            let best_chains = generated_tree.best_chains();

            if !best_chains.contains(&actual) {
                let actions_as_string: String = actions
                    .actions()
                    .iter()
                    .map(|action| action.pretty_print())
                    .collect::<Vec<_>>()
                    .list_to_string(",\n");

                let error = anyhow!(
                    r#"
                        The actual chain

                        {}

                        is not in the best chains

                        {}

                        The headers tree is
                        {}

                        The actions are

                        {}
                        "#,
                    actual.list_to_string(",\n  "),
                    best_chains.lists_to_string(",\n  ", ",\n  "),
                    generated_tree.tree(),
                    actions_as_string
                );
                return TestResult::ko(nodes, actions.clone(), error);
            }
        }
    }
    TestResult::ok(nodes, actions.clone())
}
