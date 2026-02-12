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

use crate::simulator::RunConfig;
use amaru::tests::configuration::NodeType;
use amaru::tests::setup::Node;
use amaru_consensus::headers_tree::data_generation::{Action, GeneratedActions};
use amaru_kernel::utils::string::{ListToString, ListsToString};
use amaru_ouroboros::get_best_chain_block_headers;
use amaru_protocols::store_effects::ResourceHeaderStore;
use anyhow::anyhow;
use std::fmt::{Debug, Formatter};

/// Property: at the end of the simulation, the chain built from the history of messages received
/// downstream must match one of the best chains built directly from messages coming from upstream peers.
///
/// TODO: at some point we should implement a deterministic tie breaker when multiple best chains exist
/// based on the VRF key of the received headers.
pub fn check_chain_property(nodes: Vec<Node>, actions: &GeneratedActions) -> TestResult {
    let mut test_result = TestResult {
        generated_actions: actions.clone(),
        nodes,
        result: Ok(()),
    };

    for node in test_result.nodes.iter() {
        if node.config.node_type == NodeType::DownstreamNode {
            tracing::info!(node_id = %node.config.listen_address, "checking chain property for downstream node");

            let store = node
                .running
                .resources()
                .get::<ResourceHeaderStore>()
                .expect("A ResourceHeaderStore must be available")
                .clone();
            let actual = get_best_chain_block_headers(store.clone());
            tracing::info!(node_id = %node.config.listen_address, blocks_nb = %actual.len(), "retrieved the best dowstream block headers");

            let generated_tree = test_result.generated_actions.generated_tree();
            let best_chains = generated_tree.best_chains();

            if !best_chains.contains(&actual) {
                let actions_as_string: String = test_result
                    .actions()
                    .iter()
                    .map(|action| action.pretty_print())
                    .collect::<Vec<_>>()
                    .list_to_string(",\n");

                let error = Err(anyhow!(
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
                ));
                test_result.result = error;
                return test_result;
            }
        }
    }
    test_result
}

pub struct TestResult {
    generated_actions: GeneratedActions,
    nodes: Vec<Node>,
    result: anyhow::Result<()>,
}

impl Debug for TestResult {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "TestResult {{ result: {:?}, actions: {:?}, nodes: {:?} }}",
            self.result, self.generated_actions, self.nodes
        )
    }
}

impl TestResult {
    pub fn generated_actions(&self) -> GeneratedActions {
        self.generated_actions.clone()
    }

    pub fn actions(&self) -> Vec<Action> {
        self.generated_actions.actions()
    }

    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }

    pub fn is_err(&self) -> bool {
        self.result.is_err()
    }

    pub fn set_test_failure(
        &mut self,
        run_config: &RunConfig,
        test_number: u32,
        number_of_shrinks: u32,
    ) {
        if let Err(reason) = &self.result {
            let mut formatted = String::new();
            self.actions()
                .iter()
                .enumerate()
                .for_each(|(index, action)| {
                    formatted += &format!(
                        "{:5}.  {:?} ==> {:?}\n",
                        index,
                        action.peer(),
                        action.hash()
                    )
                });

            let error = format!(
                "\nFailed after {test_number} tests\n\n \
                Minimised input ({number_of_shrinks} shrinks):\n\n \
                History:\n\n{}\n \
                Error message:\n  {}\n\n \
                Seed: {}\n",
                formatted, reason, run_config.seed
            );
            self.result = Err(anyhow!("Test failed: {}", error));
        }
    }
}
