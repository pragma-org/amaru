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

use amaru_ouroboros_traits::in_memory_consensus_store::InMemConsensusStore;
use proptest::proptest;

use crate::headers_tree::data_generation::{GeneratedTree, any_tree_of_headers, config_begin};

mod assertions;
mod harness;
mod state;

use harness::Harness;
use state::{AssertionFailure, TestState};

const DEPTH: usize = 10;
const TEST_CASES_NB: u32 = 10_000;

proptest! {
    #![proptest_config(config_begin().no_shrink().with_cases(TEST_CASES_NB).end())]
    #[test]
    fn test_chain_selection(
        generated_tree in any_tree_of_headers(DEPTH),
        seed in 0..u64::MAX,
    ) {
        let result = execute_messages(generated_tree.clone(), seed);

        if let Some(error) = result {
            eprintln!("\nproperty test failure\n{}\n", error);
            eprintln!("\nseed\n{}\n", seed);
            eprintln!("\nheaders tree\n{}\n", generated_tree.tree());
            eprintln!("\ndownstream outputs\n{:#?}\n", error.outputs);
            eprintln!("\nsent hashes\n{:?}\n", error.sent_hashes);
            eprintln!("\nvalidated invalid blocks\n{:?}\n", error.invalid_blocks);
            proptest::prop_assert!(false, "{error}");
        }
    }
}

/// Generate select_chain messages and execute them.
///
/// The messages model: tips from upstream, fetch next commands, block validation results and restarts.
///
/// At each step, assert that the current output matches the best non-invalidated tip derivable
/// from the sent headers and validation results so far.
///
fn execute_messages(generated_tree: GeneratedTree, seed: u64) -> Option<AssertionFailure> {
    let store = Arc::new(InMemConsensusStore::default());
    let mut harness = Harness::new(store.clone());
    let mut state = TestState::new(generated_tree, seed, store.clone());

    while let Some(action) = state.next_action() {
        let outputs = harness.execute_action(&action, &state);
        state.assert_best_known_tip(&outputs)?;
    }

    None
}
