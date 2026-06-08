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

use amaru_kernel::{MemoizedTransactionOutput, PoolId, PoolParams, StakeCredential, TransactionInput};

use crate::context::AccountState;

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
pub struct LedgerState {
    utxos: Utxos,
    accounts: Accounts,
    pools: Pools,
}

#[allow(dead_code)]
pub struct LedgerDelta {
    utxo_delta: MapDelta<TransactionInput, MemoizedTransactionOutput>,
    account_delta: MapDelta<StakeCredential, AccountState>,
    pool_delta: MapDelta<PoolId, PoolParams>,
}

#[allow(dead_code)]
pub type Utxos = BTreeMap<Rc<TransactionInput>, Rc<MemoizedTransactionOutput>>;
#[allow(dead_code)]
pub type Accounts = BTreeMap<Rc<StakeCredential>, Rc<AccountState>>;
#[allow(dead_code)]
pub type Pools = BTreeMap<Rc<PoolId>, Rc<PoolParams>>;

pub struct MapDelta<K: Ord, V> {
    upserted: BTreeMap<Rc<K>, Rc<V>>,
    removed: BTreeSet<Rc<K>>,
}

impl<K: Ord, V> Apply for MapDelta<K, V> {
    type Target = BTreeMap<Rc<K>, Rc<V>>;

    fn apply(self, base: &Self::Target) -> Self::Target {
        let MapDelta { upserted, removed } = self;
        let mut target = base.clone();

        for key in &removed {
            target.remove(key);
        }

        target.extend(upserted);
        target
    }
}

impl<K: Ord, V: PartialEq> Delta for BTreeMap<Rc<K>, Rc<V>> {
    type Delta = MapDelta<K, V>;

    fn delta(&self, other: &Self) -> Self::Delta {
        let upserted = self
            .iter()
            .filter(|(key, value)| other.get(*key) != Some(*value))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();

        let removed = other.keys().filter(|key| !self.contains_key(*key)).cloned().collect();

        MapDelta { upserted, removed }
    }
}

impl Apply for LedgerDelta {
    type Target = LedgerState;

    fn apply(self, base: &Self::Target) -> Self::Target {
        Self::Target {
            utxos: self.utxo_delta.apply(&base.utxos),
            accounts: self.account_delta.apply(&base.accounts),
            pools: self.pool_delta.apply(&base.pools),
        }
    }
}

impl Delta for LedgerState {
    type Delta = LedgerDelta;
    fn delta(&self, other: &Self) -> Self::Delta {
        Self::Delta {
            utxo_delta: self.utxos.delta(&other.utxos),
            account_delta: self.accounts.delta(&other.accounts),
            pool_delta: self.pools.delta(&other.pools),
        }
    }
}
