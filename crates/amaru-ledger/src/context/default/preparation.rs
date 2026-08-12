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
    ConstitutionalCommitteeMemberStatus, DRep, DRepRegistration, GovernanceAction, MemoizedTransactionOutput, PoolId,
    ProposalId, ProposalSlim, ProposalsRoots, StakeCredential, TransactionInput, drep,
};
use amaru_observability::debug_span;

use crate::{
    context::{
        AccountState, CCMember, ContextHydratationError, DefaultValidationContext, PoolVrfs, PreparationContext,
        PrepareAccountsSlice, PrepareCommitteeSlice, PrepareDRepsSlice, PreparePoolsSlice, PrepareProposalsSlice,
        PrepareUtxoSlice, ProposalStateSlim, UnresolvedInputPolicy,
    },
    state::volatile::{Bind, Existence, VolatileDB, VolatileState, VrfOccupancy},
    store::{ReadStore, columns::pools_vrf},
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
    pub pools_vrf: BTreeSet<&'a pools_vrf::Key>,
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
            pools_vrf: BTreeSet::new(),
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

    fn require_vrf_key_hash(&mut self, vrf: &'a pools_vrf::Key) {
        self.pools_vrf.insert(vrf);
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
            PoolVrfs = <VolatileDB as VolatileState>::PoolVrfs,
            VrfKeyHash = <VolatileDB as VolatileState>::VrfKeyHash,
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
            resolve_vrf_key_hashes(volatile, db, self.pools_vrf.into_iter().copied())?,
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

/// Resolves pools, materializing a [`PoolVrfs`] for each of the provided `pool_ids` that exists.
///
/// The result may be smaller than the argument: a pool could be registering for the first time in
/// this very block. Each entry carries the pool's VRF key hashes — the effective current key and
/// any pending re-registration's — which the pv11 uniqueness check compares candidate
/// registrations against. Both keys resolve `volatile -> stable`: the volatile window settles the
/// keys it established itself, or that the pending boundary transition activated or discarded,
/// and defers to the stable row otherwise.
fn resolve_pools(
    volatile: &impl VolatileState<
        Pool = <VolatileDB as VolatileState>::Pool,
        PoolVrfs = <VolatileDB as VolatileState>::PoolVrfs,
    >,
    db: &impl ReadStore,
    mut keys: impl Iterator<Item = PoolId>,
) -> Result<BTreeMap<PoolId, PoolVrfs>, ContextHydratationError> {
    debug_span!(ledger::validation_context::pools::HYDRATE).in_scope(|| {
        let mut from_volatile = 0;
        let mut from_db = 0;

        let pools = keys.try_fold(BTreeMap::new(), |mut pools, pool_id| {
            let existence = volatile.resolve_pool(pool_id);

            if matches!(existence, Existence::Gone) {
                return Ok(pools);
            }

            let vrfs = volatile.resolve_pool_vrfs(pool_id);

            // The stable row is needed both when it decides existence and when the volatile
            // window has no verdict on the current key: a pool predating the window, whose
            // registration is only found in its row.
            let row = if matches!(existence, Existence::Unknown) || matches!(vrfs.current, Existence::Unknown) {
                db.pool(&pool_id).map_err(ContextHydratationError::ResolvePools)?
            } else {
                None
            };

            match existence {
                Existence::Exists(()) => from_volatile += 1,
                Existence::Unknown if row.is_some() => from_db += 1,
                // not registered anywhere; a pool possibly registering in this very block
                Existence::Unknown => return Ok(pools),
                Existence::Gone => unreachable!("short-circuited above"),
            }

            let current = match (vrfs.current, &row) {
                (Existence::Exists(vrf), _) => vrf,
                (Existence::Unknown, Some(row)) => row.current_params.vrf,
                (Existence::Unknown, None) | (Existence::Gone, _) => {
                    unreachable!("pool {pool_id} exists, yet its current parameters are nowhere to be found ?!")
                }
            };

            let pending = match vrfs.pending {
                Existence::Exists(vrf) => Some(vrf),
                // settled by the pending boundary transition: activated or discarded
                Existence::Gone => None,
                Existence::Unknown => {
                    row.and_then(|row| row.pending_certificates.last_registration().map(|params| params.vrf))
                }
            };

            pools.insert(pool_id, PoolVrfs { current, pending });

            Ok(pools)
        })?;

        let span = tracing::Span::current();
        span.record("from_volatile", from_volatile);
        span.record("from_db", from_db);

        Ok(pools)
    })
}

/// The in-use subset of the provided candidate VRF key hashes, layering the volatile window's
/// occupancy verdicts over the stable counters: a volatile claim or release settles a key, and a
/// deferred verdict weighs the stable count against the pending boundary decrements it carries.
fn resolve_vrf_key_hashes(
    volatile: &impl VolatileState<VrfKeyHash = VrfOccupancy>,
    db: &impl ReadStore,
    mut keys: impl Iterator<Item = pools_vrf::Key>,
) -> Result<BTreeSet<pools_vrf::Key>, ContextHydratationError> {
    debug_span!(ledger::validation_context::vrf_key_hashes::HYDRATE).in_scope(|| {
        let mut from_volatile = 0;
        let mut from_db = 0;

        let in_use = keys.try_fold(BTreeSet::new(), |mut in_use, vrf| {
            match volatile.resolve_vrf_key_hash(&vrf) {
                VrfOccupancy::Claimed => {
                    from_volatile += 1;
                    in_use.insert(vrf);
                }

                VrfOccupancy::Released => {
                    from_volatile += 1;
                }

                VrfOccupancy::Deferred(decrements) => {
                    let count = db
                        .vrf_key_hash(&vrf)
                        .map_err(ContextHydratationError::ResolveVrfKeyHashes)?
                        .unwrap_or_default();

                    if count > decrements {
                        from_db += 1;
                        in_use.insert(vrf);
                    }
                }
            }

            Ok(in_use)
        })?;

        let span = tracing::Span::current();
        span.record("from_volatile", from_volatile);
        span.record("from_db", from_db);

        Ok(in_use)
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
    use std::{iter, sync::Arc};

    use amaru_kernel::{
        CertificatePointer, Epoch, Hash, PREPROD_DEFAULT_PROTOCOL_PARAMETERS, PoolParams, any_certificate_pointer,
        any_pool_params, utils::tests::run_strategy,
    };
    use test_case::test_case;

    use super::*;
    use crate::{
        epoch_transition::{GovernanceUpdates, PoolsEpochTransitionUpdates},
        state::volatile::{AnchoredVolatileFragment, VolatileDB, VolatileSequence},
        store::{self, columns::pools},
    };

    /// A stable-store stub holding a single pool row and the VRF occupancy counters.
    #[derive(Default)]
    struct StubStore {
        pool: Option<pools::Row>,
        vrf_counts: BTreeMap<pools_vrf::Key, u64>,
    }

    impl ReadStore for StubStore {
        fn pool(&self, _pool: &PoolId) -> store::Result<Option<pools::Row>> {
            Ok(self.pool.clone())
        }

        fn vrf_key_hash(&self, vrf: &pools_vrf::Key) -> store::Result<Option<pools_vrf::Value>> {
            Ok(self.vrf_counts.get(vrf).copied())
        }
    }

    fn key(tag: u8) -> pools_vrf::Key {
        Hash::new([tag; 32])
    }

    fn tag_of(vrf: pools_vrf::Key) -> u8 {
        vrf.as_ref()[0]
    }

    fn pool_id() -> PoolId {
        Hash::new([1; 28])
    }

    fn params(vrf_tag: u8) -> PoolParams {
        PoolParams { id: pool_id(), vrf: key(vrf_tag), ..run_strategy(any_pool_params()) }
    }

    fn row(current_tag: u8, pending_tag: Option<u8>) -> pools::Row {
        let mut row =
            pools::Row::new(run_strategy(any_certificate_pointer(u64::MAX)), 500_000_000, params(current_tag));
        if let Some(tag) = pending_tag {
            row.pending_certificates.append(params(tag));
        }
        row
    }

    /// What a block did to `pool_id()`: registered it as a brand-new pool or re-registered it,
    /// each with the tagged VRF key.
    #[derive(Clone, Copy)]
    enum Reg {
        New(u8),
        ReReg(u8),
    }

    fn volatile_with(reg: Option<Reg>) -> VolatileDB {
        let mut volatile = VolatileDB::default();
        if let Some(reg) = reg {
            let mut block = AnchoredVolatileFragment::fixture(10, 1);
            let tag = match reg {
                Reg::New(tag) => {
                    block.fragment.pools_current_vrf.produce(pool_id(), key(tag));
                    tag
                }
                Reg::ReReg(tag) => {
                    block.fragment.pools_pending_vrf.produce(pool_id(), key(tag));
                    tag
                }
            };
            block.fragment.pools.register(pool_id(), Arc::new((params(tag), CertificatePointer::default(), 0)));
            volatile.push_back(block);
        }
        volatile
    }

    #[test_case(None, None => None; "unknown everywhere: the pool does not exist")]
    #[test_case(None, Some((7, None)) => Some((7, None)); "untouched in the window: both keys come from the row")]
    #[test_case(None, Some((7, Some(8))) => Some((7, Some(8))); "the row's pending registration surfaces")]
    #[test_case(Some(Reg::New(7)), None => Some((7, None)); "a brand-new pool resolves without a row")]
    #[test_case(Some(Reg::ReReg(8)), Some((7, None)) => Some((7, Some(8))); "an in-window re-registration pends over the row")]
    #[test_case(Some(Reg::ReReg(9)), Some((7, Some(8))) => Some((7, Some(9))); "an in-window re-registration supersedes the row's pending")]
    fn resolve_pools_layering(reg: Option<Reg>, row_spec: Option<(u8, Option<u8>)>) -> Option<(u8, Option<u8>)> {
        let volatile = volatile_with(reg);
        let db = StubStore { pool: row_spec.map(|(current, pending)| row(current, pending)), ..StubStore::default() };

        let pools = resolve_pools(&volatile, &db, iter::once(pool_id())).unwrap();

        pools.get(&pool_id()).map(|vrfs| (tag_of(vrfs.current), vrfs.pending.map(tag_of)))
    }

    /// What a block did to `key(7)`'s occupancy.
    #[derive(Clone, Copy)]
    enum VrfAct {
        Claim,
        Release,
    }

    #[test_case(None, None => false; "an unknown key with no counter is free")]
    #[test_case(None, Some(1) => true; "a stable counter occupies the key")]
    #[test_case(Some(VrfAct::Claim), None => true; "a volatile claim occupies the key")]
    #[test_case(Some(VrfAct::Release), Some(5) => false; "a volatile release shadows the stale counter")]
    fn resolve_vrf_key_hashes_layering(act: Option<VrfAct>, count: Option<u64>) -> bool {
        let mut volatile = VolatileDB::default();
        if let Some(act) = act {
            let mut block = AnchoredVolatileFragment::fixture(10, 1);
            match act {
                VrfAct::Claim => block.fragment.pools_vrf.produce(key(7), ()),
                VrfAct::Release => block.fragment.pools_vrf.consume(key(7)),
            }
            volatile.push_back(block);
        }
        let db =
            StubStore { vrf_counts: count.map(|count| (key(7), count)).into_iter().collect(), ..StubStore::default() };

        resolve_vrf_key_hashes(&volatile, &db, iter::once(key(7))).unwrap().contains(&key(7))
    }

    #[test]
    fn resolve_vrf_key_hashes_weighs_boundary_decrements_against_the_stable_count() {
        // A pool retiring at the pending boundary transition decrements its key's occupancy: a
        // count of 1 frees the key, while a grandfathered count of 2 keeps it occupied.
        let mut volatile = VolatileDB::default();
        let mut pool = row(7, None);
        pool.pending_certificates.append(Epoch::from(1));
        let mut updates = PoolsEpochTransitionUpdates::default();
        updates.tick_pool(Epoch::from(1), pool);
        volatile.transition(
            None,
            updates,
            GovernanceUpdates::default(PREPROD_DEFAULT_PROTOCOL_PARAMETERS.clone()),
            0,
            |_| true,
        );

        let occupied = |count| {
            let db = StubStore { vrf_counts: BTreeMap::from([(key(7), count)]), ..StubStore::default() };
            resolve_vrf_key_hashes(&volatile, &db, iter::once(key(7))).unwrap().contains(&key(7))
        };

        assert!(!occupied(1), "a single decrement frees a count of 1");
        assert!(occupied(2), "a grandfathered count of 2 stays occupied through one decrement");
    }

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
            type PoolVrfs = ();
            type VrfKeyHash = ();
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
