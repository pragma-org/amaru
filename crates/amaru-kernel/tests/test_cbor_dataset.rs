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

mod common;

use amaru_kernel::protocol_version::PROTOCOL_VERSION_10;
pub use common::*;

/// Conformance against the CBOR corpus published by <https://github.com/r2rationality/cardano-cbor-dataset>.
/// Fetch it with:
/// ```text
/// make fetch-cbor-dataset
/// ```
///
/// If the data is missing, the test just prints a warning.
///
/// Set `AMARU_TEST_REPORT_DIRECTORY=<directory>` to also write the per-rule counters and the failures to a JSON file
/// that will be used to report the Amaru node conformance.
#[test]
fn test_cbor_dataset() {
    let Some(test_configuration) = TestConfiguration::create(PROTOCOL_VERSION_10).unwrap() else {
        return;
    };
    let mut test_results = TestResults::new();

    for (rule, round_trip) in RULES {
        let results = check_rule(&test_configuration, rule, round_trip).expect("failed to check the rule");
        test_results.append(results);
    }

    // Don't fail the test if some failures are acknowledged in the acknowledged-failures.toml file.
    test_results.acknowledge_failures(&test_configuration);

    report(&test_configuration, &test_results).expect("failed to report the results");

    // Fail the test if some failures are not acknowledged in the acknowledged-failures.toml file
    // or if some acknowledgements are now obsolete.
    check_unacknowledged_failures(&test_results);
}
