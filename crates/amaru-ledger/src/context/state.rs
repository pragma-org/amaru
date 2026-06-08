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

use std::{
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};

use amaru_kernel::{
    Anchor, Ballot, BallotId, ComparableProposalId, DRepRegistration, Lovelace, MemoizedTransactionOutput, PoolId,
    PoolParams, Proposal, StakeCredential, TransactionInput,
};

use crate::context::AccountState;

/// A type that can produce a patch describing the transition from another
/// instance of itself.
pub trait Diffable: Sized {
    type Patch: Patch<State = Self>;

    /// Compute a patch from `base` to `self`.
    fn diff(&self, base: &Self) -> Self::Patch;

    /// Compute both forward and reverse patches.
    fn diff_pair(&self, base: &Self) -> PatchPair<Self::Patch> {
        PatchPair { forward: self.diff(base), undo: base.diff(self) }
    }
}

/// A patch that can be applied to a state and composed with later patches.
pub trait Patch: Sized {
    type State;

    /// Apply this patch to a base state.
    fn apply(&self, base: &Self::State) -> Self::State;

    /// Extend this patch with a later patch.
    ///
    /// After:
    ///
    ///     S0 --> S1 --> S2
    ///
    /// the resulting patch represents:
    ///
    ///     S0 ---------> S2
    ///
    fn compose(&mut self, next: &Self);
}

/// A forward and reverse patch pair.
///
/// Useful for volatile state and rollback handling.
pub struct PatchPair<P> {
    pub forward: P,
    pub undo: P,
}

pub struct LedgerState {
    pub utxos: Utxos,
    pub accounts: Accounts,
    pub pools: Pools,
    pub dreps: DReps,
    pub proposals: Proposals,
    pub votes: Votes,
    pub fees: Lovelace,
}

pub struct LedgerPatch {
    pub utxos: MapPatch<TransactionInput, MemoizedTransactionOutput>,
    pub accounts: MapPatch<StakeCredential, AccountState>,
    pub pools: MapPatch<PoolId, PoolParams>,
    pub dreps: MapPatch<StakeCredential, DRepState>,
    pub proposals: MapPatch<ComparableProposalId, Proposal>,
    pub votes: MapPatch<BallotId, Ballot>,
    pub fees: Lovelace,
}

#[derive(PartialEq)]
pub struct DRepState {
    pub anchor: Anchor,
    pub registration: DRepRegistration,
}

pub type Utxos = BTreeMap<Rc<TransactionInput>, Rc<MemoizedTransactionOutput>>;

pub type Accounts = BTreeMap<Rc<StakeCredential>, Rc<AccountState>>;

pub type Pools = BTreeMap<Rc<PoolId>, Rc<PoolParams>>;

pub type DReps = BTreeMap<Rc<StakeCredential>, Rc<DRepState>>;

pub type Proposals = BTreeMap<Rc<ComparableProposalId>, Rc<Proposal>>;

pub type Votes = BTreeMap<Rc<BallotId>, Rc<Ballot>>;

/// Represents the difference between two maps.
pub struct MapPatch<K: Ord, V> {
    pub upserted: BTreeMap<Rc<K>, Rc<V>>,
    pub removed: BTreeSet<Rc<K>>,
}

impl<K: Ord, V> Default for MapPatch<K, V> {
    fn default() -> Self {
        Self { upserted: BTreeMap::new(), removed: BTreeSet::new() }
    }
}

impl<K: Ord, V> MapPatch<K, V> {
    pub fn is_removed(&self, key: &K) -> bool {
        self.removed.contains(key)
    }

    /// Lookup using overlay semantics, where both upserted and removed keys shadow the base map.
    pub fn lookup<'a>(&'a self, base: &'a BTreeMap<Rc<K>, Rc<V>>, key: &K) -> Option<&'a V> {
        if self.removed.contains(key) {
            return None;
        }

        self.upserted.get(key).or_else(|| base.get(key)).map(|value| value.as_ref())
    }
}

impl<K, V> Patch for MapPatch<K, V>
where
    K: Ord,
{
    type State = BTreeMap<Rc<K>, Rc<V>>;

    fn apply(&self, base: &Self::State) -> Self::State {
        let mut target = base.clone();

        for key in &self.removed {
            target.remove(key);
        }

        target.extend(self.upserted.clone());

        target
    }

    fn compose(&mut self, next: &Self) {
        for key in &next.removed {
            self.upserted.remove(key);
            self.removed.insert(key.clone());
        }

        for (key, value) in &next.upserted {
            self.removed.remove(key);
            self.upserted.insert(key.clone(), value.clone());
        }
    }
}

/// Helper trait for diffing maps into a MapPatch.
pub trait MapDiffExt<K: Ord, V: PartialEq> {
    fn diff_map(&self, base: &Self) -> MapPatch<K, V>;
}

impl<K, V> MapDiffExt<K, V> for BTreeMap<Rc<K>, Rc<V>>
where
    K: Ord,
    V: PartialEq,
{
    fn diff_map(&self, base: &Self) -> MapPatch<K, V> {
        let upserted = self
            .iter()
            .filter(|(key, value)| base.get(*key) != Some(*value))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();

        let removed = base.keys().filter(|key| !self.contains_key(*key)).cloned().collect();

        MapPatch { upserted, removed }
    }
}

impl Patch for LedgerPatch {
    type State = LedgerState;

    fn apply(&self, base: &LedgerState) -> LedgerState {
        LedgerState {
            utxos: self.utxos.apply(&base.utxos),
            accounts: self.accounts.apply(&base.accounts),
            pools: self.pools.apply(&base.pools),
            dreps: self.dreps.apply(&base.dreps),
            proposals: self.proposals.apply(&base.proposals),
            votes: self.votes.apply(&base.votes),
            fees: base.fees + self.fees,
        }
    }

    fn compose(&mut self, next: &Self) {
        self.utxos.compose(&next.utxos);
        self.accounts.compose(&next.accounts);
        self.pools.compose(&next.pools);
        self.dreps.compose(&next.dreps);
        self.proposals.compose(&next.proposals);
        self.votes.compose(&next.votes);

        self.fees += next.fees;
    }
}

impl Diffable for LedgerState {
    type Patch = LedgerPatch;

    fn diff(&self, base: &Self) -> Self::Patch {
        LedgerPatch {
            utxos: self.utxos.diff_map(&base.utxos),
            accounts: self.accounts.diff_map(&base.accounts),
            pools: self.pools.diff_map(&base.pools),
            dreps: self.dreps.diff_map(&base.dreps),
            proposals: self.proposals.diff_map(&base.proposals),
            votes: self.votes.diff_map(&base.votes),
            fees: self.fees - base.fees,
        }
    }
}
