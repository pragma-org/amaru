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
    CertificatePointer, ConstitutionalCommitteeMemberStatus, DRep, DRepRegistration, Epoch, Lovelace,
    MemoizedTransactionOutput, PoolId, ProposalId, StakeCredential, TransactionInput,
};

use crate::{
    context::{ProposalState, ProposalStateSlim},
    state::volatile::{
        AccountBind, CommitteeMemberBind, DRepBind, DiffSet, Empty, Existence, VolatileFragment, VolatilePoolVrfs,
    },
    store::columns::pools_vrf,
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
/// [`IndexedBind`].
type Committee = IndexedBind<StakeCredential, ConstitutionalCommitteeMemberStatus, Epoch, Empty>;

/// For Pools, it is sufficient to count registrations or de-registrations. This is because, we only
/// need the aggregate to know whether a pool was registered or not. Since both registrations and
/// de-registrations are deferred by *at least* one epoch, and because the aggregate is single-epoch
/// by design, then necessary a registration or de-registration is an indication that the pool
/// exists.
///
/// When cleaning up fragments, we can simply decrement and remove once we reach 0.
type Pools = IndexedEpochReg<PoolId>;

/// The window's per-pool *current* VRF keys, projecting Haskell's `psStakePools`: an entry exists
/// only for pools whose current parameters were established in-window, i.e. brand-new
/// registrations. See [`IndexedSet`].
type PoolsCurrentVrf = IndexedSet<PoolId, pools_vrf::Key>;

/// The window's per-pool *pending* VRF keys, projecting Haskell's `psFutureStakePoolParams`: an
/// entry exists for pools re-registered in-window, whose new parameters only activate at the next
/// epoch boundary. A pool may re-register repeatedly, so the per-key history in [`IndexedSet`] is
/// what keeps stabilization exact.
type PoolsPendingVrf = IndexedSet<PoolId, pools_vrf::Key>;

/// The window's VRF key hash occupancy, indexed per key so each one's per-fragment history is
/// retracted exactly on stabilization. Within a single epoch every change is a set-to-1 claim or a
/// release. But, unlike pool existence, occupancy is not monotonic-additive:
/// a re-registration *releases* the pending key it supersedes, so a key may be
/// claimed and released repeatedly within the window. See [`IndexedSet`].
type VrfKeyHashes = IndexedSet<pools_vrf::Key, ()>;

/// A collapse/folded sequence of `crate::volatile::VolatileFragment` which can be cleaned up
/// incrementally.
#[derive(Debug, Default)]
#[cfg_attr(feature = "test-utils", derive(Clone))]
pub struct VolatileAggregate {
    utxo: DiffSet<TransactionInput, Arc<MemoizedTransactionOutput>>,
    pools: Pools,
    pools_current_vrf: PoolsCurrentVrf,
    pools_pending_vrf: PoolsPendingVrf,
    pools_vrf: VrfKeyHashes,
    accounts: Accounts,
    dreps: DReps,
    committee: Committee,
    withdrawals: BTreeSet<StakeCredential>,
    proposals: BTreeMap<ProposalId, Arc<ProposalState>>,
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

    /// This aggregate's verdict on a pool's VRF keys, each per the newest fragment that touched
    /// it: `current` is established by a brand-new registration, `pending` by a re-registration;
    /// `Unknown` defers to the stable row.
    pub fn resolve_pool_vrfs(&self, pool_id: PoolId) -> VolatilePoolVrfs {
        VolatilePoolVrfs {
            current: self.pools_current_vrf.get(&pool_id).copied(),
            pending: self.pools_pending_vrf.get(&pool_id).copied(),
        }
    }

    /// This aggregate's verdict on a VRF key hash's occupancy: claimed (`Exists`), released
    /// (`Gone`), or untouched (`Unknown`), per the newest fragment that touched it.
    pub fn resolve_vrf_key_hash(&self, vrf: &pools_vrf::Key) -> Existence<()> {
        self.pools_vrf.get(vrf).copied()
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

    /// This aggregate's verdict on all CC members.
    pub fn resolve_cc_members<'a>(
        &'a self,
    ) -> impl Iterator<Item = (&'a StakeCredential, Existence<CommitteeMemberBind<'a>>)> {
        self.committee.iter()
    }

    /// This aggregate's view of a governance proposal. Proposals are add-only in a block, so this is
    /// `Exists` or `Unknown`; pruning only happens at the boundary.
    pub fn resolve_proposal(&self, id: &ProposalId) -> Existence<ProposalStateSlim> {
        match self.proposals.get(id) {
            Some(state) => Existence::Exists(ProposalStateSlim::from(state.as_ref())),
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
            pools_current_vrf,
            pools_pending_vrf,
            pools_vrf,
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
        self.pools_current_vrf.extend(pools_current_vrf);
        self.pools_pending_vrf.extend(pools_pending_vrf);
        self.pools_vrf.extend(pools_vrf);
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
    /// - `pools_current_vrf`, `pools_pending_vrf`: per-key history like `committee`. A pool may
    ///   re-register repeatedly within the window, each time overwriting its pending key, and only
    ///   its ordered verdicts make retracting the front exact.
    /// - `pools_vrf`: per-key history like `committee`. Occupancy is *not* monotonic-additive the
    ///   way pool existence is so a key may be claimed and released repeatedly within the window, and only its ordered
    ///   verdicts make retracting the front exact.
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
            pools_current_vrf,
            pools_pending_vrf,
            pools_vrf,
            votes: _,
        } = fragment;

        self.utxo.remove(utxo);

        assert!(
            self.pools.remove(pools),
            "removed a fragment touching onr or more key(s) abstent from the pool aggregate ?!"
        );

        assert!(
            self.pools_current_vrf.remove(pools_current_vrf),
            "removed a fragment touching one or more key(s) absent from the pool current vrf aggregate ?!"
        );

        assert!(
            self.pools_pending_vrf.remove(pools_pending_vrf),
            "removed a fragment touching one or more key(s) absent from the pool pending vrf aggregate ?!"
        );

        assert!(
            self.pools_vrf.remove(pools_vrf),
            "removed a fragment touching one or more key(s) absent from the vrf key hash aggregate ?!"
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
