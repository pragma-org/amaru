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

use std::collections::BTreeMap;

use minicbor::decode;

use crate::{TestKey, TestOutcome, normalize_shape};

/// Rules for data types whose bytes are hashed. For those data types the,
/// where the encoding is part of the format since a different encoding is a different hash.
/// This means that the re-encoding of such a value must match the original bytes exactly.
const BYTE_EXACT: &[&str] = &[
    "auxiliary_data",
    "block",
    "cost_models",
    "datum_option",
    "header",
    "header_body",
    "native_script",
    "plutus_data",
    "redeemers",
    "script",
    "transaction",
    "transaction_body",
    "transaction_witness_set",
];

/// Recorded when amaru decodes a sample that should be rejected (even if it conforms to the CDDL).
const DECODED_BUT_SHOULD_BE_REJECTED: &str = "decoded successfully, should be rejected";

pub struct TestResults {
    pub failures: BTreeMap<TestKey, String>,
    pub per_rule: BTreeMap<String, TestOutcome>,
}

impl TestResults {
    pub fn new() -> Self {
        Self { failures: BTreeMap::new(), per_rule: BTreeMap::new() }
    }

    pub fn is_successful(&self) -> bool {
        self.failures.is_empty() && self.per_rule.values().all(|outcome| outcome.is_successful())
    }

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

    pub fn total(&self) -> usize {
        self.per_rule.values().map(|t| t.generated_total + t.zap_must_be_rejected_expected).sum()
    }

    pub fn append(&mut self, other: TestResults) {
        self.failures.extend(other.failures);
        for (rule, outcome) in other.per_rule {
            self.per_rule.entry(rule).or_insert_with(TestOutcome::default).merge(outcome);
        }
    }

    pub fn update(
        &mut self,
        test_key: &TestKey,
        actual_cbor: Result<Vec<u8>, decode::Error>,
        expected_cbor: Option<Vec<u8>>,
    ) {
        let rule = test_key.rule();
        let outcome = self.per_rule.entry(rule.into()).or_insert_with(TestOutcome::default);
        if test_key.is_zapped() {
            outcome.zap_must_be_rejected_expected += 1;
            if actual_cbor.is_err() {
                outcome.zap_must_be_rejected_actual += 1;
            } else {
                self.failures.insert(test_key.clone(), DECODED_BUT_SHOULD_BE_REJECTED.to_string());
            }
        } else {
            outcome.generated_total += 1;

            if let Some(expected) = expected_cbor {
                outcome.generated_decoded_reencoded_expected += 1;
                match actual_cbor {
                    Ok(re_encoded) => {
                        let exact = re_encoded == expected;
                        let agrees = exact
                            || (!BYTE_EXACT.contains(&rule)
                                && normalize_shape(&re_encoded) == normalize_shape(&expected));
                        if agrees {
                            outcome.generated_decoded_reencoded_actual += 1;
                        } else {
                            self.failures.insert(
                                test_key.clone(),
                                format!(
                                    "re-encoding differs from the cbor reference: expected\n\n{}\n\ngot\n\n{}\n\n",
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
            } else {
                outcome.generated_must_be_rejected_expected += 1;
                if actual_cbor.is_err() {
                    outcome.generated_must_be_rejected_actual += 1;
                } else {
                    self.failures.insert(test_key.clone(), DECODED_BUT_SHOULD_BE_REJECTED.to_string());
                }
            }
        }
    }
}
