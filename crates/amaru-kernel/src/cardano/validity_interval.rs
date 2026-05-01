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

use std::fmt;

use crate::Slot;

/// The range of slots during which a transaction is valid.
///
/// A ValidityInterval is inclusive at the lower bound, but exclusive at the upper bound.
///
/// If `None`, the `lower_bound` is -inf.
/// If `None`, the `upper_bound is inf.
#[derive(Debug, Default, Clone, Copy)]
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

    /// Determine if this [`ValidityInterval`] includes a given [`Slot`]
    pub fn includes(&self, slot: Slot) -> bool {
        match (self.lower_bound, self.upper_bound) {
            (None, None) => true,
            (None, Some(upper_bound)) => slot.as_u64() < upper_bound,
            (Some(lower_bound), None) => slot.as_u64() >= lower_bound,
            (Some(lower_bound), Some(upper_bound)) => slot.as_u64() >= lower_bound && slot.as_u64() < upper_bound,
        }
    }
}

impl fmt::Display for ValidityInterval {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let lower = self.lower_bound.map(|s| s.to_string()).unwrap_or_else(|| "-inf".into());
        let upper = self.upper_bound.map(|s| s.to_string()).unwrap_or_else(|| "inf".into());
        write!(f, "[{lower}, {upper})")
    }
}
