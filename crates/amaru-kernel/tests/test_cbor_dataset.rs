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
/// Set `AMARU_TEST_REPORT_DIRECTORY=<directory>` to also write the per-rule counters and the failures to a JSON file.
#[test]
fn test_cbor_dataset() {
    let test_configuration =
        TestConfiguration::create(PROTOCOL_VERSION_10).expect("failed to create test configuration");
    let mut test_results = TestResults::new();

    for (rule, round_trip) in RULES {
        let results = check_rule(&test_configuration, rule, round_trip).expect("failed to check the rule");
        test_results.append(results);
    }

    report(&test_configuration, &test_results).expect("failed to report the results");
}

fn check_rule(
    test_configuration: &TestConfiguration,
    rule: &str,
    round_trip: &RoundTrip,
) -> anyhow::Result<TestResults> {
    let tests = test_configuration.read_tests_for(&rule)?;
    let mut test_results = TestResults::new();

    for test_key in tests {
        let to_decode = test_key.read_bytes()?;
        let expected_cbor = test_key.read_expected_cbor()?;
        let actual_cbor = round_trip(&to_decode, test_configuration.protocol_version());
        test_results.update(&test_key, actual_cbor, expected_cbor);
    }
    Ok(test_results)
}
