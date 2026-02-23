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
use amaru::tests::nodes::Nodes;
use amaru_consensus::headers_tree::data_generation::GeneratedActions;
use anyhow::anyhow;
use parking_lot::Mutex;
use pure_stage::trace_buffer::TraceBuffer;
use std::fmt::{Debug, Formatter};
use std::sync::Arc;

/// Result for the execution of one test on a set of nodes.
/// This result can be used as an input during shrinking with a list of actions that becomes
/// smaller and smaller.
pub struct TestResult {
    nodes: Nodes,
    generated_actions: GeneratedActions,
    result: anyhow::Result<()>,
    number_of_shrinks: u32,
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
    /// Create a successful test result
    pub fn ok(nodes: Nodes, generated_actions: GeneratedActions) -> Self {
        TestResult {
            nodes,
            generated_actions,
            result: Ok(()),
            number_of_shrinks: 0,
        }
    }

    /// Create a failed test result
    pub fn ko(nodes: Nodes, generated_actions: GeneratedActions, error: anyhow::Error) -> Self {
        TestResult {
            nodes,
            generated_actions,
            result: Err(error),
            number_of_shrinks: 0,
        }
    }

    /// Set the number of shrinks eventually executed to get that result
    pub fn set_number_of_shrinks(mut self, number_of_shrinks: u32) -> Self {
        self.number_of_shrinks = number_of_shrinks;
        self
    }

    /// Return the trace buffer associated to the node under test
    pub fn get_node_under_test_trace_buffer(&self) -> anyhow::Result<Arc<Mutex<TraceBuffer>>> {
        for node in &self.nodes {
            if node.is_node_under_test() {
                return Ok(node.trace_buffer());
            }
        }
        Err(anyhow!("the node under test was not found"))
    }

    /// Return true for a successful result
    pub fn is_ok(&self) -> bool {
        self.result.is_ok()
    }

    /// Return true for a failed result
    pub fn is_err(&self) -> bool {
        self.result.is_err()
    }

    /// Make a full error message for this result, based on the run config and the current test number.
    pub fn finalize_result(self, run_config: &RunConfig, test_number: u32) -> anyhow::Result<()> {
        if let Err(reason) = &self.result {
            let mut formatted = String::new();
            self.generated_actions
                .actions()
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
                Minimised input ({} shrinks):\n\n \
                History:\n\n{}\n \
                Error message:\n  {}\n\n \
                Seed: {}\n",
                self.number_of_shrinks, formatted, reason, run_config.seed
            );
            Err(anyhow!("Test failed: {}", error))
        } else {
            Ok(())
        }
    }
}
