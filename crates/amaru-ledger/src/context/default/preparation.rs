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
    ConstitutionalCommitteeMemberStatus, DRep, DRepRegistration, GovernanceAction, Hash, MemoizedTransactionOutput,
    PoolId, PoolSlim, ProposalId, ProposalSlim, ProposalsRoots, StakeCredential, TransactionInput, drep, size::VRF_KEY,
};
use amaru_observability::debug_span;

use crate::{
    context::{
        AccountState, CCMember, ContextHydratationError, DefaultValidationContext, PreparationContext,
        PrepareAccountsSlice, PrepareCommitteeSlice, PrepareDRepsSlice, PreparePoolsSlice, PrepareProposalsSlice,
        PrepareUtxoSlice, ProposalStateSlim, UnresolvedInputPolicy,
    },
    state::volatile::{Bind, Existence, VolatileDB, VolatileState},
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
    pub dreps: BTreeSet<Cow<'a, StakeCredential>>,
    pub drep_delegations: BTreeSet<&'a DRep>,
    pub committee: BTreeSet<&'a StakeCredential>,
    pub committee_voters: BTreeSet<StakeCredential>,
    pub proposals: BTreeSet<ProposalId>,
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
            committee_voters: BTreeSet::new(),
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
    fn require_drep(&mut self, drep: Cow<'a, StakeCredential>) {
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

    fn require_committee_voter(&mut self, hot_credential: StakeCredential) {
        self.committee_voters.insert(hot_credential);
    }
}

impl<'a> PrepareProposalsSlice<'a> for DefaultPreparationContext<'a> {
    fn require_proposal(&mut self, id: &'a ProposalId) {
        self.proposals.insert(*id);
    }
}

// -------------------------------------------------------------------------------------------------
// Context hydratation
// -------------------------------------------------------------------------------------------------

impl<'block> DefaultPreparationContext<'block> {
    pub fn into_validation_context<'volatile>(
        self,
        policy: UnresolvedInputPolicy,
        proposal_roots: ProposalsRoots,
        volatile: &'volatile impl VolatileState<
            TransactionOutput<'volatile> = <VolatileDB as VolatileState>::TransactionOutput<'volatile>,
            Pool = <VolatileDB as VolatileState>::Pool,
            Account<'volatile> = <VolatileDB as VolatileState>::Account<'volatile>,
            DRep<'volatile> = <VolatileDB as VolatileState>::DRep<'volatile>,
            CCMembers<'volatile> = <VolatileDB as VolatileState>::CCMembers<'volatile>,
            Proposal = <VolatileDB as VolatileState>::Proposal,
        >,
        db: &impl ReadStore,
    ) -> Result<DefaultValidationContext, ContextHydratationError> {
        let treasury = volatile.resolve_treasury(&db.pots().map_err(ContextHydratationError::ResolvePots)?);

        Ok(DefaultValidationContext::new(
            resolve_inputs(volatile, db, policy, self.utxo.into_iter())?,
            resolve_pools(volatile, db, self.pools.into_iter().copied())?,
            resolve_vrf_keys(volatile, db)?,
            resolve_accounts(volatile, db, self.accounts.into_iter())?,
            resolve_dreps(
                volatile,
                db,
                self.dreps
                    .into_iter()
                    .map(Cow::into_owned)
                    .chain(self.drep_delegations.into_iter().filter_map(drep::to_stake_credential)),
            )?,
            resolve_committee(volatile, db, self.committee, self.committee_voters)?,
            resolve_proposals(volatile, db, self.proposals.into_iter())?,
            proposal_roots,
            treasury,
            volatile.protocol_parameters().protocol_version,
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
fn resolve_inputs<'block, 'volatile>(
    volatile: &'volatile impl VolatileState<
        TransactionOutput<'volatile> = <VolatileDB as VolatileState>::TransactionOutput<'volatile>,
    >,
    db: &impl ReadStore,
    policy: UnresolvedInputPolicy,
    mut keys: impl Iterator<Item = &'block TransactionInput>,
) -> Result<BTreeMap<TransactionInput, MemoizedTransactionOutput>, ContextHydratationError> {
    debug_span!(ledger::validation_context::inputs::HYDRATE).in_scope(|| {
        let mut from_volatile = 0;
        let mut from_db = 0;

        let utxos = keys.try_fold(BTreeMap::new(), |mut acc, input| -> Result<_, ContextHydratationError> {
            let output = match volatile.resolve_input(input) {
                Existence::Gone => None,
                Existence::Exists(output) => {
                    from_volatile += 1;
                    Some(output.to_owned())
                }
                Existence::Unknown => {
                    db.utxo(input).map_err(ContextHydratationError::ResolveInputs)?.inspect(|_| from_db += 1)
                }
            };

            match (output, &policy) {
                (None, UnresolvedInputPolicy::Defer) => Ok(acc),
                (None, UnresolvedInputPolicy::Reject) => Err(ContextHydratationError::UnknownInput(*input)),
                (Some(output), _) => {
                    acc.insert(*input, output);
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

/// Resolves vrf counters from the stable store and pending pools updates.
fn resolve_vrf_keys(
    _volatile: &impl VolatileState<Pool = <VolatileDB as VolatileState>::Pool>,
    _db: &impl ReadStore,
) -> Result<BTreeMap<Hash<VRF_KEY>, u64>, ContextHydratationError> {
    unimplemented!("resolve_vrf_keys")
}

/// Resolves pools, confirming the existence of the provided `pool_ids`.
///
/// Returns the subset of `pool_ids` that exist in our ledger state. This may be smaller than the
/// argument: a pool could be registering for the first time in this very block.
///
/// Importantly, we only need existence, not the pool state. VRF-key uniqueness (pv11+) will be
/// enforced globally via a `vrf -> pool_id` index.
fn resolve_pools(
    volatile: &impl VolatileState<Pool = <VolatileDB as VolatileState>::Pool>,
    db: &impl ReadStore,
    mut keys: impl Iterator<Item = PoolId>,
) -> Result<BTreeMap<PoolId, PoolSlim>, ContextHydratationError> {
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

        Ok(unimplemented!("resolve_pools/PoolSlim"))
    })
}

/// The materialized [`AccountState`] for each existing credential, layering volatile over stable.
/// Structural fields resolve `volatile -> stable` (a `Gone` tombstone skips the stale stable
/// entry); the reward balance folds in the overlay credit and volatile withdrawals via
/// [`VolatileDB::resolve_reward_balance`].
fn resolve_accounts<'block, 'volatile>(
    volatile: &'volatile impl VolatileState<Account<'volatile> = <VolatileDB as VolatileState>::Account<'volatile>>,
    db: &impl ReadStore,
    mut keys: impl Iterator<Item = Cow<'block, StakeCredential>>,
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
                        deposit: *deposit,
                        pool: left.to_option(None),
                        drep: right.to_option(None),
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
                            pool: left.owned().into_option(row.pool),
                            drep: right.owned().into_option(row.drep),
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
fn resolve_dreps<'volatile>(
    volatile: &'volatile impl VolatileState<DRep<'volatile> = <VolatileDB as VolatileState>::DRep<'volatile>>,
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
                    dreps.insert(credential, registration.to_owned());
                    Ok(dreps)
                }

                // An anchor-only update carries no registration, so the record still lives below;
                // the anchor itself is metadata outside `DRepRegistration` and isn't materialized here.
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

/// The materialized [`CCMember`] for each existing credential, layering the volatile window over the
/// stable store; a `Gone` tombstone skips the stale stable entry.
///
/// A certificate names its member by the store's own key, so `cold_credentials` are resolved
/// directly. A vote names it by hot credential, which is indexed nowhere, so the whole committee has
/// to be materialized and matched.
fn resolve_committee<'block, 'volatile>(
    volatile: &'volatile impl VolatileState<CCMembers<'volatile> = <VolatileDB as VolatileState>::CCMembers<'volatile>>,
    db: &impl ReadStore,
    cold_credentials_in_certificates: BTreeSet<&'block StakeCredential>,
    hot_credentials_in_votes: BTreeSet<StakeCredential>,
) -> Result<BTreeMap<StakeCredential, CCMember>, ContextHydratationError> {
    debug_span!(ledger::validation_context::committee::HYDRATE).in_scope(|| {
        let mut cc_members = BTreeMap::new();

        // NOTE: No need to reach for the stable store if no context is needed.
        if cold_credentials_in_certificates.is_empty() && hot_credentials_in_votes.is_empty() {
            return Ok(cc_members);
        }

        let mut volatile_cc_members = volatile.resolve_cc_members();

        let mut gone_but_requested: BTreeSet<StakeCredential> = BTreeSet::new();

        for (cold_credential, row) in db.iter_cc_members().map_err(ContextHydratationError::ResolveCommittee)? {
            let for_certificates = cold_credentials_in_certificates.contains(&cold_credential);

            let for_votes = |status: Option<&ConstitutionalCommitteeMemberStatus>| {
                status.and_then(|st| st.as_hot_credential()).is_some_and(|hot| hot_credentials_in_votes.contains(hot))
            };

            match volatile_cc_members.remove(&cold_credential) {
                Some(Existence::Unknown) | None => {
                    if for_certificates || for_votes(row.status.as_ref()) {
                        cc_members.insert(cold_credential, row);
                    }
                }

                Some(Existence::Exists(Bind { left: volatile_status, right: valid_until, .. })) => {
                    let status = volatile_status.to_option(row.status.as_ref());
                    if for_certificates || for_votes(status.as_ref()) {
                        cc_members.insert(
                            cold_credential,
                            CCMember { status, valid_until: valid_until.to_option(row.valid_until.as_ref()) },
                        );
                    }
                }

                Some(Existence::Gone) => {
                    if for_certificates {
                        gone_but_requested.insert(cold_credential);
                    }
                }
            }
        }

        // Resolve any remaining CC members. This can happen when new members are recently added
        // following an epoch boundary, but not yet available in the stable store. In which case,
        // the volatile contains all the information we know about those members.
        for (cold_credential, existence) in volatile_cc_members.into_iter() {
            match existence {
                Existence::Exists(bind) => {
                    cc_members.insert(
                        *cold_credential,
                        CCMember { status: bind.left.to_option(None), valid_until: bind.right.to_option(None) },
                    );
                }

                Existence::Gone | Existence::Unknown => {
                    if cold_credentials_in_certificates.contains(cold_credential) {
                        gone_but_requested.insert(*cold_credential);
                    }
                }
            }
        }

        // NOTE: Scanning proposals when resolving committee
        //
        // In case where the member is requested for certificate but is Gone, we must
        // still scan the existing governance proposals for any UpdateCommittee action
        // that would be adding the member. Those are allowed to appear in certificates
        // for both resignation and hot credential delegation.
        //
        // We need not to scan the volatile db here because we correctly record a
        // default cc member when seeing such a proposal. So the volatile _already_
        // contains the information and a member that is Gone in the epoch transition,
        // but reinstated by a recent proposal would show up as `Exists`.
        //
        // When the proposal becomes stable, the default binding also gets removed from
        // the volatile (unless superseded by a more recent one) but the proposal is now
        // reachable through the stable store.
        if !gone_but_requested.is_empty() {
            for (_, row) in db.iter_proposals().map_err(ContextHydratationError::ResolveCommittee)? {
                if let GovernanceAction::UpdateCommittee(_, _, added, _) = row.proposal.gov_action {
                    for (cold_credential, _) in
                        added.into_iter().filter(|(candidate, _)| gone_but_requested.contains(candidate))
                    {
                        cc_members.entry(cold_credential).or_default();
                    }
                }
            }
        }

        Ok(cc_members)
    })
}

/// The [`ProposalStateSlim`] for each existing id, layering the ongoing block over the volatile DB over
/// the stable store; a `Gone` tombstone (boundary pruning) skips the stale stable entry.
///
/// Expiry is read from whichever layer answers, never re-derived: it is stamped once at submission
/// from the lifetime in force then, so a later change to `gov_action_lifetime` must neither extend
/// nor shorten an action already on the chain.
pub fn resolve_proposals(
    volatile: &impl VolatileState<Proposal = <VolatileDB as VolatileState>::Proposal>,
    db: &impl ReadStore,
    mut keys: impl Iterator<Item = ProposalId>,
) -> Result<BTreeMap<ProposalId, ProposalStateSlim>, ContextHydratationError> {
    debug_span!(ledger::validation_context::proposals::HYDRATE).in_scope(|| {
        let mut from_volatile = 0;
        let mut from_db = 0;

        let proposals = keys.try_fold(BTreeMap::new(), |mut proposals, id| {
            match volatile.resolve_proposal(&id) {
                // pruned at the boundary; skip
                Existence::Gone => Ok(proposals),

                // newly proposed in the volatile
                Existence::Exists(proposal) => {
                    from_volatile += 1;
                    proposals.insert(id, proposal);

                    Ok(proposals)
                }

                // not in the volatile; resolve from stable
                Existence::Unknown => {
                    if let Some(row) = db.proposal(&id).map_err(ContextHydratationError::ResolveProposals)? {
                        from_db += 1;
                        proposals.insert(
                            id,
                            ProposalStateSlim {
                                action: ProposalSlim::from(&row.proposal.gov_action),
                                valid_until: row.valid_until,
                            },
                        );
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

#[cfg(test)]
mod tests {
    #[cfg(test)]
    mod resolve_committee {
        use std::collections::BTreeMap;

        use amaru_kernel::{
            ConstitutionalCommitteeMemberStatus, Epoch, GovernanceAction, Proposal, ProposalId, StakeCredential,
            any_proposal, any_proposal_id, any_rational_number, any_stake_credential, utils::tests::run_strategy,
        };

        use super::super::resolve_committee;
        use crate::{
            context::CCMember,
            state::volatile::{Bind, CommitteeMemberBind, Empty, Existence, Resettable, VolatileState},
            store::{
                ReadStore, StoreError,
                columns::{cc_members, proposals},
            },
        };

        struct Mock {
            volatile_cc_members:
                Vec<(StakeCredential, Existence<Bind<ConstitutionalCommitteeMemberStatus, Epoch, Empty>>)>,
            stable_cc_members: Vec<(StakeCredential, Option<Epoch>, Option<ConstitutionalCommitteeMemberStatus>)>,
            proposals: Vec<Proposal>,
        }

        impl VolatileState for Mock {
            type TransactionOutput<'a> = ();
            type Pool = ();
            type Account<'a> = ();
            type DRep<'a> = ();
            type Proposal = ();
            type CCMembers<'a> = BTreeMap<&'a StakeCredential, Existence<CommitteeMemberBind<'a>>>;

            fn resolve_cc_members<'a>(&'a self) -> Self::CCMembers<'a> {
                let mut map = BTreeMap::new();

                for (k, v) in &self.volatile_cc_members {
                    map.insert(k, v.as_refs());
                }

                map
            }
        }

        impl ReadStore for Mock {
            fn iter_cc_members(&self) -> Result<impl Iterator<Item = (StakeCredential, cc_members::Row)>, StoreError> {
                Ok(self.stable_cc_members.iter().copied().map(|(cold_credential, valid_until, status)| {
                    (cold_credential, cc_members::Row { valid_until, status })
                }))
            }

            fn iter_proposals(&self) -> Result<impl Iterator<Item = (ProposalId, proposals::Row)>, StoreError> {
                Ok(self.proposals.iter().map(|proposal| {
                    (
                        run_strategy(any_proposal_id()),
                        proposals::Row {
                            proposal: proposal.clone(),
                            ..run_strategy(proposals::tests::any_row(u64::MAX))
                        },
                    )
                }))
            }
        }

        pub fn any_update_committee_proposal(cold_credential: StakeCredential) -> Proposal {
            any_update_committee_proposal_with_members(vec![cold_credential])
        }

        pub fn any_update_committee_proposal_with_members(cold_credentials: Vec<StakeCredential>) -> Proposal {
            let gov_action = GovernanceAction::UpdateCommittee(
                Default::default(),
                Default::default(),
                TryFrom::try_from(
                    cold_credentials
                        .into_iter()
                        .map(|cold_credential| (cold_credential, Default::default()))
                        .collect::<Vec<_>>(),
                )
                .unwrap(),
                run_strategy(any_rational_number()),
            );

            Proposal { gov_action, ..run_strategy(any_proposal()) }
        }

        #[test]
        fn recently_evicted_cc_members_still_in_proposals_are_resolved_for_certificates() {
            let cold_credential: StakeCredential = run_strategy(any_stake_credential());

            let mock = Mock {
                volatile_cc_members: vec![(cold_credential, Existence::Gone)],
                stable_cc_members: vec![(cold_credential, Some(Epoch::default()), None)],
                proposals: vec![any_update_committee_proposal(cold_credential)],
            };

            let context = resolve_committee(&mock, &mock, From::from([&cold_credential]), Default::default()).unwrap();

            assert_eq!(context.get(&cold_credential), Some(&CCMember::default()))
        }

        #[test]
        fn recently_evicted_cc_members_still_in_proposals_are_not_resolved_for_votes() {
            let cold_credential: StakeCredential = run_strategy(any_stake_credential());
            let hot_credential: StakeCredential = run_strategy(any_stake_credential());

            let mock = Mock {
                volatile_cc_members: vec![(cold_credential, Existence::Gone)],
                stable_cc_members: vec![(cold_credential, Some(Epoch::default()), Some(hot_credential.into()))],
                proposals: vec![any_update_committee_proposal(cold_credential)],
            };

            let context = resolve_committee(&mock, &mock, Default::default(), From::from([hot_credential])).unwrap();

            assert!(context.is_empty())
        }

        #[test]
        fn recent_volatile_hot_delegation_is_used_in_status_resolution_of_elected_member() {
            let cold_credential: StakeCredential = run_strategy(any_stake_credential());
            let hot_credential: StakeCredential = run_strategy(any_stake_credential());

            let mock = Mock {
                volatile_cc_members: vec![(
                    cold_credential,
                    Existence::Exists(Bind { left: Resettable::Set(hot_credential.into()), ..Bind::default() }),
                )],
                stable_cc_members: vec![(cold_credential, Some(Epoch::default()), None)],
                proposals: vec![],
            };

            let context = resolve_committee(&mock, &mock, Default::default(), From::from([hot_credential])).unwrap();

            assert_eq!(
                context.get(&cold_credential),
                Some(&CCMember { status: Some(hot_credential.into()), valid_until: Some(Epoch::default()) })
            );
        }

        #[test]
        fn unelected_cc_members_with_delegation_are_resolved_for_votes() {
            let cold_credential: StakeCredential = run_strategy(any_stake_credential());
            let hot_credential: StakeCredential = run_strategy(any_stake_credential());

            let mock = Mock {
                volatile_cc_members: vec![(
                    cold_credential,
                    Existence::Exists(Bind { left: Resettable::Set(hot_credential.into()), ..Bind::default() }),
                )],
                stable_cc_members: vec![],
                proposals: vec![any_update_committee_proposal(cold_credential)],
            };

            let context = resolve_committee(&mock, &mock, Default::default(), From::from([hot_credential])).unwrap();

            assert_eq!(
                context.get(&cold_credential),
                Some(&CCMember { status: Some(hot_credential.into()), valid_until: None })
            );
        }

        #[test]
        fn all_recently_evicted_cc_members_still_in_proposals_are_resolved_for_certificates() {
            let first_cold_credential: StakeCredential = run_strategy(any_stake_credential());
            let second_cold_credential: StakeCredential = run_strategy(any_stake_credential());

            let mock = Mock {
                volatile_cc_members: vec![
                    (first_cold_credential, Existence::Gone),
                    (second_cold_credential, Existence::Gone),
                ],
                stable_cc_members: vec![
                    (first_cold_credential, Some(Epoch::default()), None),
                    (second_cold_credential, Some(Epoch::default()), None),
                ],
                proposals: vec![any_update_committee_proposal_with_members(vec![
                    first_cold_credential,
                    second_cold_credential,
                ])],
            };

            let context = resolve_committee(
                &mock,
                &mock,
                From::from([&first_cold_credential, &second_cold_credential]),
                Default::default(),
            )
            .unwrap();

            assert_eq!(context.get(&first_cold_credential), Some(&CCMember::default()));
            assert_eq!(context.get(&second_cold_credential), Some(&CCMember::default()));
        }

        #[test]
        fn stable_hot_delegation_is_resolved_for_votes_without_volatile_entry() {
            let cold_credential: StakeCredential = run_strategy(any_stake_credential());
            let hot_credential: StakeCredential = run_strategy(any_stake_credential());

            let mock = Mock {
                volatile_cc_members: vec![],
                stable_cc_members: vec![(cold_credential, Some(Epoch::default()), Some(hot_credential.into()))],
                proposals: vec![],
            };

            let context = resolve_committee(&mock, &mock, Default::default(), From::from([hot_credential])).unwrap();

            assert_eq!(
                context.get(&cold_credential),
                Some(&CCMember { status: Some(hot_credential.into()), valid_until: Some(Epoch::default()) })
            );
        }
    }
}
