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

use std::collections::{BTreeMap, BTreeSet};

use minicbor::decode;

use crate::{AcknowledgedFailure, Category, TestConfiguration, TestKey, TestOutcome, error_class, normalize_shape};

/// Rules for data types whose bytes are hashed. For those data types the,
/// where the encoding is part of the format since a different encoding is a different hash.
/// This means that the re-encoding of such a value must match the original bytes exactly.
const BYTE_EXACT: &[&str] = &[
    "auxiliary_data",
    "header",
    "native_script",
    "plutus_data",
    "redeemers",
    "transaction_body",
    "transaction_witness_set",
];

/// Recorded when amaru decodes a sample that should be rejected.
const DECODED_BUT_SHOULD_BE_REJECTED: &str = "decoded successfully, should be rejected";

/// Recorded when amaru re-encodes a sample into bytes the (normalized) reference does not agree with.
const RE_ENCODING_DIFFERS: &str = "re-encoding differs from the cbor reference";

/// The results of a test run, including the failures and the per-rule outcomes.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TestResults {
    /// Failure by test sample
    pub failures: BTreeMap<TestKey, String>,
    /// Aggregate failure by rule
    pub per_rule: BTreeMap<String, TestOutcome>,
    /// The unacknowledged failures, as a list of `(rule, class)` pairs.
    /// This is updated after all the rules have been checked.
    pub unacknowledged: Vec<(String, String)>,
    /// The acknowledged failures that are no longer observed in the current run.
    pub stale: Vec<AcknowledgedFailure>,
}

impl TestResults {
    pub fn new() -> Self {
        Self::default()
    }

    /// The test run is successful if there is no failure for any test key.
    pub fn is_successful(&self) -> bool {
        self.failures.is_empty()
    }

    /// Return the outcome a specific rule for detailed reporting
    pub fn get_test_outcome(&self, rule: &str) -> TestOutcome {
        self.per_rule.get(rule).cloned().unwrap_or(TestOutcome::default())
    }

    /// The per-rule outcomes summed into a single outcome for the whole run.
    pub fn totals(&self) -> TestOutcome {
        let mut totals = TestOutcome::default();
        for outcome in self.per_rule.values() {
            totals.merge(outcome.clone());
        }
        totals
    }

    /// Total number of samples executed, including both generated and zapped samples.
    pub fn total(&self) -> usize {
        self.per_rule.values().map(|t| t.generated_total + t.zap_must_be_rejected_expected).sum()
    }

    /// Append the results of another run to this one, merging the failures and per-rule outcomes.
    pub fn append(&mut self, other: TestResults) {
        self.failures.extend(other.failures);
        for (rule, outcome) in other.per_rule {
            self.per_rule.entry(rule).or_default().merge(outcome);
        }
    }

    /// Update the test results with the outcome of a single test sample.
    pub fn update(
        &mut self,
        test_key: &TestKey,
        actual_cbor: Result<Vec<u8>, decode::Error>,
        expected_cbor: Option<Vec<u8>>,
    ) -> anyhow::Result<()> {
        let rule = test_key.rule();
        let outcome = self.per_rule.entry(rule.into()).or_default();
        match test_key.category() {
            Category::Valid => {
                let Some(expected) = expected_cbor else {
                    anyhow::bail!("no reference bytes for the valid sample {test_key}");
                };
                outcome.generated_total += 1;
                outcome.generated_decoded_reencoded_expected += 1;
                match actual_cbor {
                    Ok(re_encoded) => {
                        let exact = re_encoded == expected;
                        let agrees = exact
                            || (!BYTE_EXACT.contains(&rule)
                                && normalize_shape(&re_encoded)? == normalize_shape(&expected)?);
                        if agrees {
                            outcome.generated_decoded_reencoded_actual += 1;
                        } else {
                            self.failures.insert(
                                test_key.clone(),
                                format!(
                                    "{RE_ENCODING_DIFFERS}\n\nexpected\n\n{}\n\ngot\n\n{}\n\n",
                                    hex::encode(&expected),
                                    hex::encode(&re_encoded)
                                ),
                            );
                        }
                    }
                    Err(e) => {
                        self.failures.insert(test_key.clone(), e.to_string());
                    }
                }
            }
            Category::InvalidGenerated => {
                outcome.generated_total += 1;
                outcome.generated_must_be_rejected_expected += 1;
                if actual_cbor.is_err() {
                    outcome.generated_must_be_rejected_actual += 1;
                } else {
                    self.failures.insert(test_key.clone(), DECODED_BUT_SHOULD_BE_REJECTED.to_string());
                }
            }
            Category::Zapped(_) => {
                outcome.zap_must_be_rejected_expected += 1;
                if actual_cbor.is_err() {
                    outcome.zap_must_be_rejected_actual += 1;
                } else {
                    self.failures.insert(test_key.clone(), DECODED_BUT_SHOULD_BE_REJECTED.to_string());
                }
            }
        }
        Ok(())
    }

    /// Mark some failures as acknowledged, and record any stale acknowledgements that are no longer observed in this run.
    pub fn acknowledge_failures(&mut self, test_configuration: &TestConfiguration) {
        // The distinct `(rule, class)` pairs the run produced, which is what acknowledgements are matched against.
        let observed: BTreeSet<(String, String)> =
            self.failures.iter().map(|(key, reason)| (key.rule().to_string(), error_class(reason))).collect();
        let acknowledged = test_configuration.acknowledged_failures();

        self.unacknowledged =
            observed.iter().filter(|(rule, class)| !acknowledged.contains(rule, class)).cloned().collect();

        // Only collect stale acknowledgements if no filter is applied, otherwise the stale list would be incomplete and misleading.
        self.stale = if test_configuration.filter().is_none() { acknowledged.stale(&observed) } else { Vec::new() };
    }
}
