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
    sync::Arc,
};

use amaru_kernel::{
    CertificatePointer, DRep, DRepRegistration, Lovelace, MemoizedTransactionOutput, PoolId, Proposal, ProposalEnum,
    ProposalId, ProposalPointer, StakeCredential, TransactionInput,
};

use crate::state::volatile::{
    AccountBind, Bind, CommitteeMemberBind, DRepBind, DiffSet, Empty, Existence, Resettable, VolatileFragment,
};

mod indexed_bind;
pub use indexed_bind::IndexedBind;

mod indexed_epoch_reg;
pub use indexed_epoch_reg::IndexedEpochReg;

mod indexed_set;
pub use indexed_set::IndexedSet;

/// The window's accounts, indexed by credential so each one's per-fragment history is retracted
/// exactly on stabilization and folded on read. See [`IndexedBind`].
type Accounts = IndexedBind<StakeCredential, (PoolId, CertificatePointer), (DRep, CertificatePointer), Lovelace>;

/// The window's DReps, indexed by credential so each one's per-fragment history is retracted exactly
/// on stabilization and folded on read. See [`IndexedBind`].
type DReps = IndexedBind<StakeCredential, Empty, Empty, DRepRegistration>;

/// The window's constitutional committee, indexed by cold credential so each member's hot-key
/// history is retracted exactly on stabilization. A member may rotate their hot key (produce then
/// produce), so a blind collapse would lose the newer key when the older fragment stabilizes. See
/// [`IndexedSet`].
type Committee = IndexedSet<StakeCredential, StakeCredential>;

/// For Pools, it is sufficient to count registrations or de-registrations. This is because, we only
/// need the aggregate to know whether a pool was registered or not. Since both registrations and
/// de-registrations are deferred by *at least* one epoch, and because the aggregate is single-epoch
/// by design, then necessary a registration or de-registration is an indication that the pool
/// exists.
///
/// When cleaning up fragments, we can simply decrement and remove once we reach 0.
type Pools = IndexedEpochReg<PoolId>;

/// A collapse/folded sequence of `crate::volatile::VolatileFragment` which can be cleaned up
/// incrementally.
#[derive(Debug, Default)]
#[cfg_attr(feature = "test-utils", derive(Clone))]
pub struct VolatileAggregate {
    utxo: DiffSet<TransactionInput, Arc<MemoizedTransactionOutput>>,
    pools: Pools,
    accounts: Accounts,
    dreps: DReps,
    committee: Committee,
    withdrawals: BTreeSet<StakeCredential>,
    proposals: BTreeMap<ProposalId, Arc<(Proposal, ProposalPointer)>>,
    fees: Lovelace,
    donations: Lovelace,
}

impl VolatileAggregate {
    /// Whether this aggregate has seen an account withdrew rewards
    pub fn has_withdrawal(&self, credential: &StakeCredential) -> bool {
        self.withdrawals.contains(credential)
    }

    /// The donations made by the fragments folded here; that is, those not yet accounted for in the
    /// stable pots. They are moved into the treasury at the epoch boundary.
    pub fn donations(&self) -> Lovelace {
        self.donations
    }
}

impl VolatileAggregate {
    pub fn resolve_input(&self, input: &'_ TransactionInput) -> Existence<&MemoizedTransactionOutput> {
        self.utxo.get(input).as_deref()
    }

    /// Whether this aggregate registered the given pool. Unregistrations
    /// do *not* affect existence: a pool stays live until it is actually retired at the epoch boundary.
    pub fn resolve_pool(&self, pool_id: PoolId) -> bool {
        self.pools.get(&pool_id)
    }

    /// This aggregate's verdict on a stake account, folding the credential's per-fragment
    /// contributions oldest to newest. Deregistration is immediate, so an `unregistered` entry is a
    /// live tombstone.
    pub fn resolve_account<'a>(&'a self, credential: &StakeCredential) -> Existence<AccountBind<'a>> {
        self.accounts.get(credential)
    }

    /// This aggregate's verdict on a DRep, folding the credential's per-fragment contributions
    /// oldest to newest. Deregistration is immediate, so a tombstone is live; an anchor-only update
    /// is a bind-only change that defers the registration to the layer below.
    pub fn resolve_drep<'a>(&'a self, credential: &StakeCredential) -> Existence<DRepBind<'a>> {
        self.dreps.get(credential)
    }

    /// This aggregate's verdict on a CC member. Resignation is immediate, so a resignation entry is a
    /// live tombstone. A delegation resolves as a bind-only update (`value: None`): no in-block cert
    /// establishes membership, so existence still defers to the layer below.
    pub fn resolve_cc_member<'a>(&'a self, credential: &StakeCredential) -> Existence<CommitteeMemberBind<'a>> {
        use Existence::*;
        use Resettable::*;

        match self.committee.get(credential) {
            Unknown => Unknown,
            Gone => Exists(Bind { left: Reset, ..Bind::default() }),
            Exists(hot) => Exists(Bind { left: Set(hot), ..Bind::default() }),
        }
    }

    /// This aggregate's view of a governance proposal. Proposals are add-only in a block, so this is
    /// `Exists` or `Unknown`; pruning only happens at the boundary.
    pub fn resolve_proposal(&self, id: &ProposalId) -> Existence<ProposalEnum> {
        match self.proposals.get(id) {
            Some(entry) => Existence::Exists(ProposalEnum::from(&entry.0.gov_action)),
            None => Existence::Unknown,
        }
    }
}

impl VolatileAggregate {
    /// Fold a `more_recent` into this aggregate, treating it as applied *after* `self`.
    /// This maintains the running aggregate of a [`crate::state::volatile::VolatileSeries`].
    pub fn add_fragment(&mut self, fragment: &VolatileFragment) {
        let VolatileFragment {
            utxo,
            pools,
            withdrawals,
            proposals,
            fees,
            donations,
            accounts,
            dreps,
            dreps_deregistrations: _,
            committee,
            votes: _,
        } = fragment;

        self.utxo.extend(utxo);
        self.pools.extend(pools);
        self.withdrawals.extend(withdrawals.iter().cloned());
        self.proposals.extend(proposals.clone());
        self.dreps.extend_with(dreps, |bind| bind.map_left(|_| Empty));
        self.committee.extend(committee);
        self.accounts.extend(accounts);

        self.fees += *fees;
        self.donations += *donations;
    }

    /// Retract the oldest fragment as it stabilizes off the front of the window, leaving this
    /// aggregate exactly equal to what re-folding the remaining fragments would produce.
    ///
    /// This exactness is what lets a series stabilize purely incrementally, with no periodic
    /// recompute to fall back on. It is not automatic: a naive collapse loses information every
    /// time two fragments merge (e.g. an account that registers, deregisters, then re-registers
    /// within the window collapses to a single verdict, and retracting the front then reads the
    /// wrong existence). Each field is therefore shaped so that retracting the front is exact,
    /// each for its own reason:
    ///
    /// - `utxo`: utxos, by definition, are unique, so cleaning up specific UTxOs can never overwrite older state.
    /// - `pools`: existence is monotonic-additive, a registration counts up and retirement is
    ///   deferred to the epoch boundary, so retracting just decrements the count this fragment added.
    /// - `accounts`, `dreps`: each credential keeps its own per-fragment history, so retracting
    ///   pops only the front of that credential's deque and a later re-registration or bind-only
    ///   update is left intact. This is what the collapse above would lose.
    /// - `committee`: same per-key history; a member may rotate their hot key (produce then
    ///   produce), and both verdicts are kept, so stabilizing the older one leaves the newer live.
    /// - `withdrawals`: only *effectful* withdrawals are recorded (the rule drops zero-value ones,
    ///   which move no rewards), and a series is epoch-homogeneous, so a credential has at most one
    ///   of them. Rewards accrue only at the epoch boundary and a withdrawal drains the whole
    ///   balance. A credential therefore appears in at most one fragment, and dropping it from the
    ///   set as that fragment retracts is exact.
    /// - `proposals`: proposal ids are globally unique, so, as with `utxo`, a removed id is never
    ///   re-added.
    /// - `fees`, `donations`: running totals, retracted by subtracting exactly what was added.
    pub fn remove_fragment(&mut self, fragment: &VolatileFragment) {
        let VolatileFragment {
            utxo,
            withdrawals,
            proposals,
            fees,
            donations,
            accounts,
            committee,
            dreps,
            dreps_deregistrations: _,
            pools,
            votes: _,
        } = fragment;

        self.utxo.remove(utxo);

        assert!(
            self.pools.remove(pools),
            "removed a fragment touching onr or more key(s) abstent from the pool aggregate ?!"
        );

        self.committee.remove(committee);

        for credential in withdrawals {
            self.withdrawals.remove(credential);
        }

        for proposal_id in proposals.keys() {
            self.proposals.remove(proposal_id);
        }

        assert!(
            self.accounts.remove(accounts),
            "removed a fragment touching one or more key(s) absent from the account aggregate ?!"
        );

        assert!(
            self.dreps.remove(dreps),
            "removed a fragment touching one or more key(s) absent from the dreps aggregate ?!"
        );

        self.fees -= *fees;
        self.donations -= *donations;
    }
}
