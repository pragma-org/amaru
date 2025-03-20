// Copyright 2025 PRAGMA
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

pub mod into_keys;
pub mod strategies;

use crate::into_keys::IntoKeys;
use std::ops::Deref;

/// An simple mempool interface to add transactions and forge blocks when needed.
pub trait Mempool<T>: Send + Sync
where
    T: Deref + Send + Sync,
    T::Target: IntoKeys,
    <T::Target as IntoKeys>::Key: Ord,
{
    /// Add a new transaction to the mempool.
    ///
    /// TODO: Have the mempool perform its own set of validations and possibly fail to add new
    /// elements. This is non-trivial, since it requires the mempool to have ways of re-validating
    /// transactions provided a slightly different context.
    ///
    /// We shall circle back to this once we've done some progress on the ledger validations and
    /// the so-called ledger slices.
    fn add(&mut self, tx: T);

    /// Take transactions out of the mempool, with the intent of forging a new block.
    ///
    /// TODO: Have this function take _constraints_, such as the block max size or max execution
    /// units and select transactions accordingly.
    fn take(&mut self) -> Vec<T>;

    /// Take note of a transaction happening outside of the mempool. This should in principle
    /// invalidate transactions within the mempool that are now considered invalid.
    fn acknowledge(&mut self, tx: &T::Target);
}
