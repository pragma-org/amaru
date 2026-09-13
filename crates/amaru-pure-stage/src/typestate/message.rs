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

//! Variant names of a [`define_messages!`](crate::define_messages) enum.

use std::collections::BTreeSet;

/// Label of a type whose `stringify!` is a remainder-graph name.
///
/// Implemented for `define_messages!` payloads and for local/plumbing inputs
/// via [`impl_label!`](crate::impl_label). Object-safe so [`labels`] can take a
/// mixed list. Use `T::LABEL` without constructing a value.
pub trait MessageLabel {
    /// `stringify!` of this payload type.
    fn label(&self) -> &'static str;
}

/// Collect payload type names. `BTreeSet` because projection looks up by name.
pub fn labels<'a>(items: impl IntoIterator<Item = &'a dyn MessageLabel>) -> BTreeSet<&'static str> {
    items.into_iter().map(MessageLabel::label).collect()
}

/// Names of the variants of a protocol message enum.
///
/// [`define_messages!`](crate::define_messages) implements this. `labels` and
/// [`label`](Self::label) are generated from the same variant list, so a new
/// variant cannot compile without appearing in both.
pub trait MessageLabels {
    /// Variant names in declaration order (`stringify!` of each variant).
    fn labels() -> &'static [&'static str];

    /// Name of this value's variant. The match is exhaustive.
    fn label(&self) -> &'static str;
}

/// Every [`MessageLabels::labels`] entry is in `table` or `unused`.
#[track_caller]
pub fn assert_message_alphabet_covered<M: MessageLabels>(
    table: impl IntoIterator<Item = &'static str>,
    unused: &[&'static str],
) {
    let table: BTreeSet<&str> = table.into_iter().collect();
    let unused: BTreeSet<&str> = unused.iter().copied().collect();
    for &label in unused.iter() {
        assert!(
            M::labels().contains(&label),
            "{label} is listed unused but is not a variant of {}",
            core::any::type_name::<M>()
        );
        assert!(!table.contains(label), "{label} is both in the spec table and listed unused");
    }
    for &label in M::labels() {
        assert!(
            table.contains(label) || unused.contains(label),
            "{label} is neither in the spec table nor listed unused"
        );
    }
}
