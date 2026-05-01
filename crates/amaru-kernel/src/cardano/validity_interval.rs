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

/// The range of slots during which a transaction is valid.
///
/// If `None`, the `lower_bound` is -inf.
/// If `None`, the `upper_bound is inf.
#[derive(Default)]
pub struct ValidityInterval {
    lower_bound: Option<u64>,
    upper_bound: Option<u64>,
}

impl ValidityInterval {
    pub fn new(lower_bound: Option<u64>, upper_bound: Option<u64>) -> Self {
        Self { lower_bound, upper_bound }
    }
    pub fn lower_bound(&self) -> &Option<u64> {
        &self.lower_bound
    }

    pub fn upper_bound(&self) -> &Option<u64> {
        &self.upper_bound
    }
}
