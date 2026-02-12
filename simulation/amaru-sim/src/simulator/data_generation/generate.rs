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
use amaru_consensus::headers_tree::data_generation::{
    GeneratedActions, any_select_chains_from_tree, any_tree_of_headers,
};
use amaru_kernel::utils::tests::run_strategy_with_rng;

/// Generates a sequence of random actions from peers on a tree of
/// headers generated with a specified depth.
///
/// TODO: since we are generating data with a `proptest` strategy the simulation framework can not
/// for now shrink the list of actions generated here. This means that if a test fails the generated data
/// will not be minimized to find a smaller failing case. The generation is deterministic though based on the
/// RNG passed as a parameter.
pub fn generate_actions(run_config: &RunConfig) -> GeneratedActions {
    let mut rng = run_config.rng();
    // Generate a tree of headers.
    let generated_tree = run_strategy_with_rng(
        &mut rng.0,
        any_tree_of_headers(run_config.generated_chain_depth),
    );

    // Generate actions corresponding to peers doing roll forwards and roll backs on the tree.
    run_strategy_with_rng(
        &mut rng.0,
        any_select_chains_from_tree(&generated_tree, &run_config.upstream_peers()),
    )
}
