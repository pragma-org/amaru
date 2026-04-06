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

// Raising the recursion limit is necessary for writing a large json! element in the
// run_simulator_with_traces test
#![recursion_limit = "256"]

use amaru_sim::simulator::{initialize_logs, make_args, run_tests};

/// Run the simulator with arguments from environment variables.
#[test]
#[ignore]
pub fn run_simulator() {
    initialize_logs();
    if let Err(e) = run_tests(make_args()) {
        panic!("Test failed!\n{}", e);
    }
}
