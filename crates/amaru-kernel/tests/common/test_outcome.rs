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

/// Outcome for testing either a single rule or a set of rules.
#[derive(Default, Clone, PartialEq, Eq, Debug, serde::Serialize)]
pub struct TestOutcome {
    /// Total number of samples generated from the CDDL (they might not be all decodable).
    pub generated_total: usize,
    /// Total number of generated samples that must be successfully decoded, re-encoded and match
    /// the reference cbor bytes.
    pub generated_decoded_reencoded_expected: usize,
    /// Total number of generated samples that have been successfully decoded, re-encoded and matched
    /// the reference cbor bytes, as expected
    pub generated_decoded_reencoded_actual: usize,
    /// Number of generated samples that must be rejected when they are decoded (even if they conform to the CDDL).
    pub generated_must_be_rejected_expected: usize,
    /// Number of generated samples that have be rejected as expected.
    pub generated_must_be_rejected_actual: usize,
    /// Number of malformed mutations that must be rejected.
    pub zap_must_be_rejected_expected: usize,
    /// Number of malformed mutations that amaru has rejected as expected.
    pub zap_must_be_rejected_actual: usize,
}

impl TestOutcome {
    pub fn merge(&mut self, other: TestOutcome) {
        self.generated_total += other.generated_total;
        self.generated_decoded_reencoded_expected += other.generated_decoded_reencoded_expected;
        self.generated_decoded_reencoded_actual += other.generated_decoded_reencoded_actual;
        self.generated_must_be_rejected_expected += other.generated_must_be_rejected_expected;
        self.generated_must_be_rejected_actual += other.generated_must_be_rejected_actual;
        self.zap_must_be_rejected_expected += other.zap_must_be_rejected_expected;
        self.zap_must_be_rejected_actual += other.zap_must_be_rejected_actual;
    }

    pub fn is_successful(&self) -> bool {
        self.generated_decoded_reencoded_expected == self.generated_decoded_reencoded_actual
            && self.generated_must_be_rejected_expected == self.generated_must_be_rejected_actual
            && self.zap_must_be_rejected_expected == self.zap_must_be_rejected_actual
    }
}
