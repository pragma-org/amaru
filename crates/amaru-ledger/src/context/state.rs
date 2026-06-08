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

use std::{
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};

use amaru_kernel::{MemoizedTransactionOutput, TransactionInput};

/// A change that can be applied to a value to produce another value of the same type.
///
/// Companion to [`Delta`]: [`Delta::delta`] computes the change between two values, and
/// [`apply`](Apply::apply) replays it onto a base value. The two operations are inverses.
///
/// ```rs
/// target.delta(&base).apply(&base) == target
/// ```
///
/// Applying consumes the change, as it describes a single transition.
#[allow(dead_code)]
pub trait Apply {
    /// The value type this change applies to and produces.
    type Target;

    /// Apply this change to the given base value, returning the result.
    fn apply(self, base: &Self::Target) -> Self::Target;
}

/// Computes the change between two values of `Self`.
///
/// Produces a [`Delta`](Delta::Delta) that [`Apply`] can replay. The receiver is the target and
/// the argument the base, so the result describes how to reconstruct the receiver from the
/// argument:
///
/// ```rs
/// target.delta(&base).apply(&base) == target
/// ```
#[allow(dead_code)]
pub trait Delta {
    /// The change produced by [`delta`](Delta::delta); consumed by [`Apply`].
    type Delta;

    /// Compute the change from the given base value to `self`.
    fn delta(&self, other: &Self) -> Self::Delta;
}

#[allow(dead_code)]
pub struct UtxoState {
    utxos: BTreeMap<Rc<TransactionInput>, Rc<MemoizedTransactionOutput>>,
}

#[allow(dead_code)]
pub struct UtxoDelta {
    added: BTreeMap<Rc<TransactionInput>, Rc<MemoizedTransactionOutput>>,
    removed: BTreeSet<Rc<TransactionInput>>,
}

#[allow(dead_code)]
pub struct LedgerState {
    utxo_state: UtxoState,
}

#[allow(dead_code)]
pub struct LedgerDelta {
    utxo_delta: UtxoDelta,
}

impl Apply for LedgerDelta {
    type Target = LedgerState;

    fn apply(self, base: &Self::Target) -> Self::Target {
        Self::Target { utxo_state: self.utxo_delta.apply(&base.utxo_state) }
    }
}

impl Delta for LedgerState {
    type Delta = LedgerDelta;
    fn delta(&self, other: &Self) -> Self::Delta {
        Self::Delta { utxo_delta: self.utxo_state.delta(&other.utxo_state) }
    }
}

impl Apply for UtxoDelta {
    type Target = UtxoState;

    fn apply(self, base: &Self::Target) -> Self::Target {
        let UtxoDelta { added, removed } = self;
        Self::Target {
            utxos: base
                .utxos
                .iter()
                .filter(|(input, _)| !removed.contains(*input))
                .map(|(input, output)| (input.clone(), output.clone()))
                .chain(added)
                .collect(),
        }
    }
}

impl Delta for UtxoState {
    type Delta = UtxoDelta;

    fn delta(&self, other: &Self) -> Self::Delta {
        let added = self
            .utxos
            .iter()
            .filter(|(input, _)| !other.utxos.contains_key(*input))
            .map(|(input, output)| (input.clone(), output.clone()))
            .collect();

        let removed = other.utxos.keys().filter(|input| !self.utxos.contains_key(*input)).cloned().collect();

        UtxoDelta { added, removed }
    }
}
