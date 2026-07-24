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

use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
};

use amaru_kernel::{
    ComparableProposalId, DRep, DRepRegistration, MemoizedTransactionOutput, PoolId, ProposalId, ProposalsRoots,
    StakeCredential, TransactionInput, drep,
};
use amaru_observability::debug_span;

use crate::{
    context::{
        AccountState, CCMember, ContextHydratationError, DefaultValidationContext, PreparationContext,
        PrepareAccountsSlice, PrepareCommitteeSlice, PrepareDRepsSlice, PreparePoolsSlice, PrepareProposalsSlice,
        PrepareUtxoSlice, UnresolvedInputPolicy,
    },
    state::{
        diff_bind::Bind,
        volatile::{AccountBind, CommitteeMemberBind, DRepBind, Existence, RewardsAtTip, VolatileState},
    },
    store::ReadStore,
};

/// An implementation of the block preparation context that's suitable for use in normal operation.
///
/// It is for now incomplete, but we'll use eventually bridge the gap between the validations and
/// the state management so that this fully replaces the current state module and child modules.
///
/// For now, there's still a bit of duplication between the modules.
#[derive(Debug, Default)]
pub struct DefaultPreparationContext<'a> {
    pub utxo: BTreeSet<&'a TransactionInput>,
    pub pools: BTreeSet<&'a PoolId>,
    pub accounts: BTreeSet<Cow<'a, StakeCredential>>,
    pub dreps: BTreeSet<&'a StakeCredential>,
    pub drep_delegations: BTreeSet<&'a DRep>,
    pub committee: BTreeSet<&'a StakeCredential>,
    pub proposals: BTreeSet<ComparableProposalId>,
}

impl DefaultPreparationContext<'_> {
    pub fn new() -> Self {
        Self {
            utxo: BTreeSet::new(),
            pools: BTreeSet::new(),
            accounts: BTreeSet::new(),
            dreps: BTreeSet::new(),
            drep_delegations: BTreeSet::new(),
            committee: BTreeSet::new(),
            proposals: BTreeSet::new(),
        }
    }
}

impl<'a> PreparationContext<'a> for DefaultPreparationContext<'a> {}

impl<'a> PrepareUtxoSlice<'a> for DefaultPreparationContext<'a> {
    fn require_input(&'_ mut self, input: &'a TransactionInput) {
        self.utxo.insert(input);
    }
}

impl<'a> PreparePoolsSlice<'a> for DefaultPreparationContext<'a> {
    fn require_pool(&mut self, pool: &'a PoolId) {
        self.pools.insert(pool);
    }
}

impl<'a> PrepareAccountsSlice<'a> for DefaultPreparationContext<'a> {
    fn require_account(&mut self, credential: Cow<'a, StakeCredential>) {
        self.accounts.insert(credential);
    }
}

impl<'a> PrepareDRepsSlice<'a> for DefaultPreparationContext<'a> {
    fn require_drep(&mut self, drep: &'a StakeCredential) {
        self.dreps.insert(drep);
    }

    fn require_drep_delegation(&mut self, drep: &'a DRep) {
        self.drep_delegations.insert(drep);
    }
}

impl<'a> PrepareCommitteeSlice<'a> for DefaultPreparationContext<'a> {
    fn require_committee_member(&mut self, cc_member: &'a StakeCredential) {
        self.committee.insert(cc_member);
    }
}

impl<'a> PrepareProposalsSlice<'a> for DefaultPreparationContext<'a> {
    fn require_proposal(&mut self, id: &'a ProposalId) {
        self.proposals.insert(ComparableProposalId::from(id.clone()));
    }
}

// -------------------------------------------------------------------------------------------------
// Context hydratation
// -------------------------------------------------------------------------------------------------

impl<'a> DefaultPreparationContext<'a> {
    pub fn into_validation_context(
        self,
        policy: UnresolvedInputPolicy,
        proposal_roots: ProposalsRoots,
        volatile: &impl VolatileState<
            Pool = Existence<()>,
            Account = (Existence<AccountBind>, RewardsAtTip),
            DRep = Existence<DRepBind>,
            CCMember = Existence<CommitteeMemberBind>,
            Proposal = Existence<()>,
        >,
        db: &impl ReadStore,
    ) -> Result<DefaultValidationContext, ContextHydratationError> {
        Ok(DefaultValidationContext::new(
            resolve_inputs(volatile, db, policy, self.utxo.into_iter())?,
            resolve_pools(volatile, db, self.pools.into_iter().copied())?,
            resolve_accounts(volatile, db, self.accounts.into_iter())?,
            resolve_dreps(
                volatile,
                db,
                self.dreps
                    .into_iter()
                    .cloned()
                    .chain(self.drep_delegations.into_iter().filter_map(drep::to_stake_credential)),
            )?,
            resolve_committee(volatile, db, self.committee.into_iter())?,
            resolve_proposals(volatile, db, self.proposals.into_iter())?,
            proposal_roots,
        ))
    }
}

// TODO: batch stable-store lookups during context prepraration
//
// Perform below db lookups in batch, and possibly within the same transaction as other required data
// pre-fetch.

/// Resolve inputs/UTxO necessary for the validation context using what was marked during
/// preparation. This search in the volatile first and reaches for the stable store if
/// necessary.
fn resolve_inputs<'a>(
    volatile: &impl VolatileState,
    db: &impl ReadStore,
    policy: UnresolvedInputPolicy,
    mut keys: impl Iterator<Item = &'a TransactionInput>,
) -> Result<BTreeMap<TransactionInput, MemoizedTransactionOutput>, ContextHydratationError> {
    debug_span!(ledger::validation_context::inputs::HYDRATE).in_scope(|| {
        let mut from_volatile = 0;
        let mut from_db = 0;

        let utxos = keys.try_fold(BTreeMap::new(), |mut acc, input| -> Result<_, ContextHydratationError> {
            let output = if volatile.has_consumed_input(input) {
                Ok(None)
            } else {
                match volatile.resolve_input(input) {
                    Some(output) => {
                        from_volatile += 1;
                        Ok(Some(output.clone()))
                    }
                    None => {
                        Ok(db.utxo(input).map_err(ContextHydratationError::ResolveInputs)?.inspect(|_| from_db += 1))
                    }
                }
            }?;

            match (output, &policy) {
                (None, UnresolvedInputPolicy::Defer) => Ok(acc),
                (None, UnresolvedInputPolicy::Reject) => Err(ContextHydratationError::UnknownInput(input.clone())),
                (Some(output), _) => {
                    acc.insert(input.clone(), output);
                    Ok(acc)
                }
            }
        });

        let span = tracing::Span::current();
        span.record("from_volatile", from_volatile);
        span.record("from_db", from_db);

        utxos
    })
}

/// Resolves pools, confirming the existence of the provided `pool_ids`.
///
/// Returns the subset of `pool_ids` that exist in our ledger state. This may be smaller than the
/// argument: a pool could be registering for the first time in this very block.
///
/// Importantly, we only need existence, not the pool state. VRF-key uniqueness (pv11+) will be
/// enforced globally via a `vrf -> pool_id` index.
fn resolve_pools(
    volatile: &impl VolatileState<Pool = Existence<()>>,
    db: &impl ReadStore,
    mut keys: impl Iterator<Item = PoolId>,
) -> Result<BTreeSet<PoolId>, ContextHydratationError> {
    debug_span!(ledger::validation_context::pools::HYDRATE).in_scope(|| {
        let mut from_volatile = 0;
        let mut from_db = 0;

        let pools = keys.try_fold(BTreeSet::new(), |mut pools, pool_id| {
            match volatile.resolve_pool(pool_id) {
                Existence::Gone => {}

                Existence::Exists(()) => {
                    pools.insert(pool_id);
                    from_volatile += 1;
                }

                Existence::Unknown => {
                    if db.pool(&pool_id).map_err(ContextHydratationError::ResolvePools)?.is_some() {
                        pools.insert(pool_id);
                        from_db += 1;
                    }
                }
            }

            Ok(pools)
        })?;

        let span = tracing::Span::current();
        span.record("from_volatile", from_volatile);
        span.record("from_db", from_db);

        Ok(pools)
    })
}

/// The materialized [`AccountState`] for each existing credential, layering volatile over stable.
/// Structural fields resolve `volatile -> stable` (a `Gone` tombstone skips the stale stable
/// entry); the reward balance folds in the overlay credit and volatile withdrawals via
/// [`VolatileDB::resolve_reward_balance`].
fn resolve_accounts<'iter>(
    volatile: &impl VolatileState<Account = (Existence<AccountBind>, RewardsAtTip)>,
    db: &impl ReadStore,
    mut keys: impl Iterator<Item = Cow<'iter, StakeCredential>>,
) -> Result<BTreeMap<StakeCredential, AccountState>, ContextHydratationError> {
    debug_span!(ledger::validation_context::accounts::HYDRATE).in_scope(|| {
        let mut from_volatile = 0;
        let mut from_db = 0;

        let accounts =
            keys.try_fold(BTreeMap::new(), |mut accounts, credential| match volatile.resolve_account(&credential) {
                (Existence::Gone, _) => Ok(accounts),

                (Existence::Exists(Bind { value: Some(deposit), left, right }), rewards_at_tip) => {
                    from_volatile += 1;

                    let state = AccountState {
                        deposit,
                        pool: left.as_borrowed().to_option(None),
                        drep: right.as_borrowed().to_option(None),
                        rewards: rewards_at_tip.into_balance(0),
                    };

                    accounts.insert(credential.into_owned(), state);

                    Ok(accounts)
                }

                (Existence::Exists(Bind { value: None, left, right }), rewards_at_tip) => {
                    if let Some(row) = db.account(&credential).map_err(ContextHydratationError::ResolveAccounts)? {
                        from_db += 1;

                        let state = AccountState {
                            deposit: row.deposit,
                            pool: left.as_borrowed().to_option(row.pool.as_ref()),
                            drep: right.as_borrowed().to_option(row.drep.as_ref()),
                            rewards: rewards_at_tip.into_balance(row.rewards),
                        };

                        accounts.insert(credential.into_owned(), state);
                    }

                    Ok(accounts)
                }

                (Existence::Unknown, rewards_at_tip) => {
                    if let Some(row) = db.account(&credential).map_err(ContextHydratationError::ResolveAccounts)? {
                        from_db += 1;

                        let state = AccountState {
                            deposit: row.deposit,
                            pool: row.pool,
                            drep: row.drep,
                            rewards: rewards_at_tip.into_balance(row.rewards),
                        };

                        accounts.insert(credential.into_owned(), state);
                    }

                    Ok(accounts)
                }
            })?;

        let span = tracing::Span::current();
        span.record("from_volatile", from_volatile);
        span.record("from_db", from_db);

        Ok(accounts)
    })
}

/// The materialized [`DRepRegistration`] for each existing credential, layering the ongoing block
/// over the volatile DB over the stable store; a `Gone` tombstone skips the stale stable entry.
/// DReps carry no balance, so there is no reward dimension; the anchor is metadata outside the
/// registration record, so a bind-only (anchor) update reads the registration from below.
fn resolve_dreps(
    volatile: &impl VolatileState<DRep = Existence<DRepBind>>,
    db: &impl ReadStore,
    mut keys: impl Iterator<Item = StakeCredential>,
) -> Result<BTreeMap<StakeCredential, DRepRegistration>, ContextHydratationError> {
    debug_span!(ledger::validation_context::dreps::HYDRATE).in_scope(|| {
        let mut from_volatile = 0;
        let mut from_db = 0;

        let dreps =
            keys.try_fold(BTreeMap::new(), |mut dreps, credential| match volatile.resolve_drep(&credential) {
                Existence::Gone => Ok(dreps),

                Existence::Exists(Bind { value: Some(registration), .. }) => {
                    from_volatile += 1;
                    dreps.insert(credential, registration);
                    Ok(dreps)
                }

                Existence::Exists(Bind { value: None, .. }) | Existence::Unknown => {
                    if let Some(row) = db.drep(&credential).map_err(ContextHydratationError::ResolveDReps)? {
                        from_db += 1;

                        let registration = DRepRegistration {
                            deposit: row.deposit,
                            registered_at: row.registered_at,
                            valid_until: row.valid_until,
                        };

                        dreps.insert(credential, registration);
                    }

                    Ok(dreps)
                }
            })?;

        let span = tracing::Span::current();
        span.record("from_volatile", from_volatile);
        span.record("from_db", from_db);

        Ok(dreps)
    })
}

/// The materialized [`CCMember`] for each existing credential, layering the ongoing block over
/// the volatile DB over the stable store; a `Gone` tombstone skips the stale stable entry. The
/// hot key resolves through the layers, but the term is set only at the boundary or in the
/// stable store, so it folds in the overlay's pending value via
/// [`VolatileDB::resolve_committee_term`].
///
// FIXME: resolve committee member credentials from pending updates
//
// a cold credential present in a pending UpdateCommittee proposal also counts as a known
// member (Haskell's `cgceCommitteeProposals`), which lets a not-yet-elected member pre-declare
// its hot key. That source needs the proposals read-path, so it is deferred until proposals are
// exposed.
fn resolve_committee<'a>(
    volatile: &impl VolatileState<CCMember = Existence<CommitteeMemberBind>>,
    db: &impl ReadStore,
    mut keys: impl Iterator<Item = &'a StakeCredential>,
) -> Result<BTreeMap<StakeCredential, CCMember>, ContextHydratationError> {
    debug_span!(ledger::validation_context::committee::HYDRATE).in_scope(|| {
        let mut from_volatile = 0;
        let mut from_db = 0;

        let cc_members = keys.try_fold(BTreeMap::new(), |mut cc_members, credential| {
            let member_opt = match volatile.resolve_cc_member(credential) {
                Existence::Gone => {
                    from_volatile += 1;
                    None
                }

                Existence::Exists(Bind { value, left: hot_credential, .. }) => {
                    if let Some(valid_until) = value {
                        from_volatile += 1;
                        Some(CCMember {
                            hot_credential: hot_credential.into_option(None),
                            valid_until: Some(valid_until),
                        })
                    } else {
                        db.cc_member(credential).map_err(ContextHydratationError::ResolveCommittee)?.map(|mut row| {
                            from_db += 1;
                            hot_credential.set_or_reset(&mut row.hot_credential);
                            CCMember { hot_credential: row.hot_credential, valid_until: row.valid_until }
                        })
                    }
                }

                Existence::Unknown => {
                    db.cc_member(credential).map_err(ContextHydratationError::ResolveCommittee)?.map(|row| {
                        from_db += 1;
                        CCMember { hot_credential: row.hot_credential, valid_until: row.valid_until }
                    })
                }
            };

            if let Some(member) = member_opt {
                cc_members.insert(credential.clone(), member);
            }

            Ok(cc_members)
        })?;

        let span = tracing::Span::current();
        span.record("from_volatile", from_volatile);
        span.record("from_db", from_db);

        Ok(cc_members)
    })
}

/// The materialized [`ProposalState`] for each existing id, layering the ongoing block over the
/// volatile DB over the stable store; a `Gone` tombstone (boundary pruning) skips the stale
/// stable entry. A proposal still in the volatile window was proposed within the last `k` blocks,
/// so its expiry is derived from its own pointer rather than read from a not-yet-written row.
pub fn resolve_proposals(
    volatile: &impl VolatileState<Proposal = Existence<()>>,
    db: &impl ReadStore,
    mut keys: impl Iterator<Item = ComparableProposalId>,
) -> Result<BTreeSet<ComparableProposalId>, ContextHydratationError> {
    debug_span!(ledger::validation_context::proposals::HYDRATE).in_scope(|| {
        let mut from_volatile = 0;
        let mut from_db = 0;

        let proposals = keys.try_fold(BTreeSet::new(), |mut proposals, id| {
            match volatile.resolve_proposal(&id) {
                // pruned at the boundary; skip
                Existence::Gone => Ok(proposals),

                // newly proposed in the volatile
                Existence::Exists(()) => {
                    from_volatile += 1;
                    proposals.insert(id);

                    Ok(proposals)
                }

                // not in the volatile; resolve from stable
                Existence::Unknown => {
                    if db.proposal(&id).map_err(ContextHydratationError::ResolveProposals)?.is_some() {
                        from_db += 1;
                        proposals.insert(id);
                    }

                    Ok(proposals)
                }
            }
        })?;

        let span = tracing::Span::current();
        span.record("from_volatile", from_volatile);
        span.record("from_db", from_db);

        Ok(proposals)
    })
}
