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

#![expect(dead_code)]

/// A type that can produce a delta describing the transition from another
/// instance of itself.
pub trait Diffable: Sized {
    type Delta: Delta<State = Self>;

    /// Compute a delta from `base` to `self`.
    fn diff(&self, base: &Self) -> Self::Delta;

    /// Compute both forward and reverse deltas.
    fn diff_pair(&self, base: &Self) -> DeltaPair<Self::Delta> {
        DeltaPair { forward: self.diff(base), undo: base.diff(self) }
    }
}

/// A delta that can be applied to a state and composed with later deltas.
pub trait Delta: Sized {
    type State;
    type Error;

    /// Apply this delta to `base` in place, returning the inverse delta that
    /// restores `base` to the value it held before the application.
    fn apply(&self, base: &mut Self::State) -> Self;

    /// Extend this delta with a later delta.
    ///
    /// After:
    ///
    /// ```text
    /// S0 --> S1 --> S2
    /// ```
    ///
    /// the resulting delta represents:
    ///
    /// ```text
    /// S0 ---------> S2
    /// ```
    fn compose(&mut self, next: &Self) -> Result<(), Self::Error>;
}

/// A forward and reverse delta pair.
///
/// Useful for volatile state and rollback handling.
pub struct DeltaPair<P> {
    pub forward: P,
    pub undo: P,
}
