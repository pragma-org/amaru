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
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    ops::DerefMut,
    sync::Arc,
};

use amaru_kernel::{
    CertificatePointer, ComparableProposalId, DRep, DRepRegistration, Lovelace, MemoizedTransactionOutput, PoolId,
    PoolParams, StakeCredential, TransactionInput,
};

use crate::state::{
    diff_bind::{DiffBind, Resettable},
    diff_epoch_reg::DiffEpochReg,
    diff_set::DiffSet,
    volatile::{AccountBind, Bind, CommitteeMemberBind, Existence, VolatileFragment},
};

type Pools = DiffEpochReg<PoolId, Arc<(PoolParams, CertificatePointer, Lovelace)>>;

type Accounts = DiffBind<StakeCredential, (PoolId, CertificatePointer), (DRep, CertificatePointer), Lovelace>;

/// A collapse/folded sequence of `crate::volatile::VolatileFragment` which can be cleaned up
/// incrementally.
#[derive(Debug, Default)]
#[cfg_attr(feature = "test-utils", derive(Clone))]
pub struct VolatileAggregate {
    utxo: DiffSet<TransactionInput, Arc<MemoizedTransactionOutput>>,
    pools: Pools,
    accounts: RefCell<Option<Accounts>>,
    dreps: DiffSet<StakeCredential, DRepRegistration>,
    dreps_deregistrations: BTreeMap<StakeCredential, CertificatePointer>,
    committee: DiffSet<StakeCredential, StakeCredential>,
    withdrawals: BTreeSet<StakeCredential>,
    proposals: BTreeSet<ComparableProposalId>,
    fees: Lovelace,
    donations: Lovelace,
}

impl VolatileAggregate {
    /// Whether this aggregate has seen an input been consumed.
    pub fn has_consumed_input(&self, input: &TransactionInput) -> bool {
        self.utxo.consumed.contains(input)
    }

    /// Whether this aggregate has seen an account withdrew rewards
    pub fn has_withdrawal(&self, credential: &StakeCredential) -> bool {
        self.withdrawals.contains(credential)
    }
}

impl VolatileAggregate {
    pub fn resolve_input(&self, input: &TransactionInput) -> Option<&MemoizedTransactionOutput> {
        self.utxo.produced.get(input).map(|output| output.as_ref())
    }

    /// Whether this aggregate registered the given pool. Unregistrations
    /// do *not* affect existence: a pool stays live until it is actually retired at the epoch boundary.
    pub fn resolve_pool(&self, pool_id: PoolId) -> bool {
        self.pools.registered.contains_key(&pool_id)
    }

    /// This aggregate's verdict on a stake account. Deregistration is immediate, so an `unregistered`
    /// entry is a live tombstone.
    pub fn resolve_account(
        &self,
        credential: &StakeCredential,
        scan: impl FnOnce() -> Accounts,
    ) -> Existence<AccountBind> {
        if let Some(accounts) = self.accounts.borrow().as_ref() {
            return accounts.lookup(credential).to_owned();
        }

        let accounts = scan();

        let account = accounts.lookup(credential).to_owned();

        self.accounts.replace(Some(accounts));

        account
    }

    /// This aggregate's verdict on a DRep. Deregistration is immediate, so an `unregistered`
    /// entry is a live tombstone.
    pub fn resolve_drep(&self, credential: &StakeCredential) -> Existence<DRepRegistration> {
        self.dreps.lookup(credential).copied()
    }

    /// This aggregate's verdict on a CC member. Resignation is immediate, so a resignation entry is a
    /// live tombstone. A delegation resolves as a bind-only update (`value: None`): no in-block cert
    /// establishes membership, so existence still defers to the layer below.
    pub fn resolve_cc_member(&self, credential: &StakeCredential) -> Existence<CommitteeMemberBind> {
        use Existence::*;
        use Resettable::*;

        match self.committee.lookup(credential) {
            Unknown => Unknown,
            Gone => Exists(Bind { left: Reset, ..Bind::default() }),
            Exists(hot) => Exists(Bind { left: Set(hot.to_owned()), ..Bind::default() }),
        }
    }

    /// This aggregate's view of a governance proposal. Proposals are add-only in a block, so this is
    /// `Exists` or `Unknown`; pruning only happens at the boundary.
    pub fn resolve_proposal(&self, id: &ComparableProposalId) -> Existence<()> {
        if self.proposals.contains(id) { Existence::Exists(()) } else { Existence::Unknown }
    }
}

impl VolatileAggregate {
    /// Fold a `more_recent`  into this aggregate, treating it as applied *after* `self`.
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
            dreps_deregistrations,
            committee,
            votes: _,
        } = fragment;

        self.utxo.extend(utxo);
        self.pools.extend(pools);
        self.withdrawals.extend(withdrawals.iter().cloned());
        self.proposals.extend(proposals.keys().cloned());
        self.dreps.extend_bind(dreps);
        self.dreps_deregistrations.extend(dreps_deregistrations.iter().map(|(k, v)| (k.clone(), *v)));
        self.committee.extend(committee);

        if !accounts.is_empty() {
            // Modify the accounts cache, if present. If not, does nothing. Accounts are only re-calculated
            // lazily when needed.
            if let Some(diff) = self.accounts.borrow_mut().deref_mut() {
                diff.append(accounts.clone());
            }
        }

        self.fees += *fees;
        self.donations += *donations;
    }

    /// A best-effort cleanup of a previous fragment in the current aggreate. This is not generally
    /// possible (with our current design), for all elements in a fragment, because we loose
    /// information each time we aggregate two fragments (a little thought exercise with account
    /// registrations and delegations should be convincing enough).
    ///
    /// But it is possible for a few types such as the `DiffSet` and the various maps. Note that
    /// not cleaning up all the data is not fundamentally wrong; but it is *leaking memory*. We
    /// just keep in memory information that we should have flushed on-disk.
    ///
    /// Yet, this is counterbalanced by the frequent rollbacks happening on Cardano (once every
    /// 10-15min due to slot battles). Rollbacks are infrequent enough and frequent enough that
    /// they are the perfect opportunity to cleanup the now-stable memory (by re-computing the
    /// aggregate from scratch). Also, because we cannot *guarantee* that rollbacks happen, we
    /// still also manually perform such a cleanup every now-and-then using a counter that gets
    /// reset for every rollback.
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
            dreps_deregistrations,
            pools: _,
            votes: _,
        } = fragment;

        self.utxo.cleanup(utxo);

        self.committee.cleanup(committee);

        for credential in dreps_deregistrations.keys() {
            self.dreps_deregistrations.remove(credential);
        }

        for credential in withdrawals {
            self.withdrawals.remove(credential);
        }

        for proposal_id in proposals.keys() {
            self.proposals.remove(proposal_id);
        }

        if !accounts.is_empty() {
            self.accounts.replace(None);
        }

        if !dreps.is_empty() {
            self.dreps.cleanup_bind(dreps)
        }

        self.fees -= *fees;
        self.donations -= *donations;
    }
}
