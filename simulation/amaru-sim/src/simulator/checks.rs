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

use amaru::tests::{node::Node, nodes::Nodes};
use amaru_consensus::headers_tree::data_generation::GeneratedActions;
use amaru_kernel::{
    Transaction,
    utils::string::{ListToString, ListsToString},
};
use amaru_ouroboros::{ResourceMempool, TxId, get_best_chain_block_headers};
use amaru_protocols::store_effects::ResourceHeaderStore;
use anyhow::anyhow;

use crate::simulator::test_result::TestResult;

/// Properties checked at the end of the simulation:
///
/// - each downstream node must have selected one of the best chains induced by the upstream peer actions;
/// - each injected transaction must have reached the node under test and all upstream peers.
///
/// TODO: at some point we should implement a deterministic tie breaker when multiple best chains exist
/// based on the VRF key of the received headers.
pub fn check_properties(nodes: Nodes, actions: &GeneratedActions, expected_tx_ids: &[TxId]) -> TestResult {
    if let Err(error) = check_chain_property(&nodes, actions) {
        return TestResult::ko(nodes, actions.clone(), error);
    }

    if let Err(error) = check_tx_submission_property(&nodes, expected_tx_ids) {
        return TestResult::ko(nodes, actions.clone(), error);
    }

    TestResult::ok(nodes, actions.clone())
}

/// Check that each downstream node ends on one of the best chains induced by the
/// upstream peer actions.
pub fn check_chain_property(nodes: &Nodes, actions: &GeneratedActions) -> anyhow::Result<()> {
    for node in nodes {
        if node.is_downstream() {
            tracing::info!(node_id = %node.node_id(), "checking chain property for downstream node");

            let store =
                node.resources().get::<ResourceHeaderStore>().expect("A ResourceHeaderStore must be available").clone();
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
                return Err(error);
            }
        }
    }
    Ok(())
}

/// Check that each injected transaction has reached the node under test and all
/// upstream peers by the end of the simulation.
pub fn check_tx_submission_property(nodes: &Nodes, expected_tx_ids: &[TxId]) -> anyhow::Result<()> {
    for node in nodes {
        if node.is_node_under_test() || node.is_upstream() {
            assert_has_all_txs(node, expected_tx_ids)?;
        }
    }
    Ok(())
}

fn assert_has_all_txs(node: &Node, expected_tx_ids: &[TxId]) -> anyhow::Result<()> {
    let mempool = node
        .resources()
        .get::<ResourceMempool<Transaction>>()
        .expect("A ResourceMempool<Transaction> must be available")
        .clone();

    let txs = mempool.get_txs_for_ids(expected_tx_ids);
    if txs.len() != expected_tx_ids.len() {
        return Err(anyhow!("node {} did not receive all expected transactions", node.node_id()));
    }

    Ok(())
}
