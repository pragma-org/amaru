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
/// If `None`, the `upper_bound` is +inf.
#[derive(Debug, Default, Clone, Copy)]
pub struct ValidityInterval {
    lower_bound: Option<Slot>,
    upper_bound: Option<Slot>,
}

impl ValidityInterval {
    pub fn new(lower_bound: Option<Slot>, upper_bound: Option<Slot>) -> Self {
        Self { lower_bound, upper_bound }
    }

    // Construct a fully bounded validity interval. Note that the upper bound is exclusive.
    pub fn between(lower_bound: Slot, upper_bound: Slot) -> Self {
        Self { lower_bound: Some(lower_bound), upper_bound: Some(upper_bound) }
    }

    // Construct a validity interval from an inclusive lower bound.
    pub fn after(lower_bound: Slot) -> Self {
        Self { lower_bound: Some(lower_bound), upper_bound: None }
    }

    /// Construct a validity interval from an exclusive upper bound. "Strictly" refers to how the
    /// upper bound is generally treated as exclusive.
    pub fn strictly_before(upper_bound: Slot) -> Self {
        Self { lower_bound: None, upper_bound: Some(upper_bound) }
    }

    pub fn lower_bound(&self) -> Option<Slot> {
        self.lower_bound
    }

    pub fn upper_bound(&self) -> Option<Slot> {
        self.upper_bound
    }

    /// Determine if this [`ValidityInterval`] includes a given [`Slot`]
    ///
    // NOTE: about validity interval bounds
    //
    // - The lower bound is inclusive
    // - The upper bound is exclusive (starting from protocol v9, inclusive before that)
    pub fn includes(&self, slot: Slot) -> bool {
        match (self.lower_bound, self.upper_bound) {
            (None, None) => true,
            (None, Some(upper_bound)) => slot < upper_bound,
            (Some(lower_bound), None) => slot >= lower_bound,
            (Some(lower_bound), Some(upper_bound)) => slot >= lower_bound && slot < upper_bound,
        }
    }
}

impl fmt::Display for ValidityInterval {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let lower = self.lower_bound.map(|s| s.to_string()).unwrap_or_else(|| "-∞".to_string());
        let upper = self.upper_bound.map(|s| s.to_string()).unwrap_or_else(|| "+∞".to_string());
        write!(f, "[{lower}, {upper})")
    }
}

#[cfg(test)]
mod tests {
    use test_case::test_case;

    use super::*;

    #[test_case(ValidityInterval::default() => "[-∞, +∞)"; "unbounded")]
    #[test_case(ValidityInterval::after(Slot::from(0)) => "[0, +∞)"; "lower bound only")]
    #[test_case(ValidityInterval::strictly_before(Slot::from(42)) => "[-∞, 42)"; "upper bound only")]
    #[test_case(ValidityInterval::between(Slot::from(42), Slot::from(84)) => "[42, 84)"; "both bounds")]
    #[test_case(ValidityInterval::new(Some(Slot::from(1)), Some(Slot::from(1))) => "[1, 1)"; "empty interval")]
    fn display_validity_interval(validity_interval: ValidityInterval) -> String {
        validity_interval.to_string()
    }
}
