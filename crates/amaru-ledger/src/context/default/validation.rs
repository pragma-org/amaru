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
    collections::{BTreeMap, BTreeSet},
    marker::PhantomData,
    mem,
    sync::Arc,
};

use amaru_kernel::{
    Anchor, Ballot, BallotId, CertificatePointer, ComparableProposalId, DRep, DRepRegistration, Epoch, Hash, Lovelace,
    MemoizedPlutusData, MemoizedScript, MemoizedTransactionOutput, Mint, PoolId, PoolParams, Proposal, ProposalId,
    ProposalPointer, RequiredScript, StakeCredential, TransactionInput, Value, Vote, Voter,
    cardano::value::Balance,
    size::{DATUM, KEY, SCRIPT},
};
use amaru_observability::trace_span;

use crate::{
    context::{
        AccountState, AccountsSlice, BalanceSlice, CCMember, CommitteeSlice, DRepsSlice, DelegateError, PoolsSlice,
        PotsSlice, ProposalsSlice, RegisterError, UnregisterError, UpdateError, UtxoSlice, ValidationContext,
        WitnessSlice, blanket_known_datums, blanket_known_scripts,
    },
    governance::ratification::ProposalsRoots,
    state::volatile::VolatileFragment,
};

#[derive(Debug)]
pub struct DefaultValidationContext {
    utxo: BTreeMap<TransactionInput, MemoizedTransactionOutput>,
    pools: BTreeSet<PoolId>,
    accounts: BTreeMap<StakeCredential, AccountState>,
    dreps: BTreeMap<StakeCredential, DRepRegistration>,
    committee: BTreeMap<StakeCredential, CCMember>,
    proposals: BTreeSet<ComparableProposalId>,
    proposals_roots: ProposalsRoots,
    state: VolatileFragment,
    known_scripts: BTreeMap<Hash<SCRIPT>, TransactionInput>,
    known_datums: BTreeMap<Hash<DATUM>, TransactionInput>,
    required_signers: BTreeSet<Hash<KEY>>,
    required_scripts: BTreeSet<RequiredScript>,
    required_supplemental_datums: BTreeSet<Hash<DATUM>>,
    required_bootstrap_roots: BTreeSet<Hash<28>>,
    balance: Balance,
}

impl DefaultValidationContext {
    pub fn new(
        utxo: BTreeMap<TransactionInput, MemoizedTransactionOutput>,
        pools: BTreeSet<PoolId>,
        accounts: BTreeMap<StakeCredential, AccountState>,
        dreps: BTreeMap<StakeCredential, DRepRegistration>,
        committee: BTreeMap<StakeCredential, CCMember>,
        proposals: BTreeSet<ComparableProposalId>,
        proposals_roots: ProposalsRoots,
    ) -> Self {
        Self {
            utxo,
            pools,
            accounts,
            dreps,
            committee,
            proposals,
            proposals_roots,
            state: VolatileFragment::default(),
            required_signers: BTreeSet::default(),
            known_scripts: BTreeMap::new(),
            known_datums: BTreeMap::new(),
            required_scripts: BTreeSet::default(),
            required_supplemental_datums: BTreeSet::default(),
            required_bootstrap_roots: BTreeSet::default(),
            balance: Balance::default(),
        }
    }
}

impl From<DefaultValidationContext> for VolatileFragment {
    fn from(ctx: DefaultValidationContext) -> VolatileFragment {
        ctx.state
    }
}

impl ValidationContext for DefaultValidationContext {
    type FinalState = VolatileFragment;
}

impl PotsSlice for DefaultValidationContext {
    fn add_fees(&mut self, fees: Lovelace) {
        self.state.fees += fees;
    }

    fn add_donation(&mut self, donation: Lovelace) {
        self.state.donations += donation;
    }
}

impl UtxoSlice for DefaultValidationContext {
    fn lookup(&self, input: &TransactionInput) -> Option<&MemoizedTransactionOutput> {
        self.utxo.get(input).or_else(|| self.state.utxo.produced.get(input).map(|output| output.as_ref()))
    }

    fn consume(&mut self, input: TransactionInput) {
        self.utxo.remove(&input);
        self.state.utxo.consume(input)
    }

    fn produce(&mut self, input: TransactionInput, output: MemoizedTransactionOutput) {
        self.state.utxo.produce(input, Arc::new(output))
    }
}

impl PoolsSlice for DefaultValidationContext {
    /// Whether the given pool exists in the resolved ledger state (including pools registered
    /// earlier within the same block).
    fn exists(&self, pool: PoolId) -> bool {
        self.pools.contains(&pool) || self.state.pools.registered.contains_key(&pool)
    }

    /// FIXME: In ProtocolVersion 11, we must also check for uniqueness of the VRF key when registering pools
    ///
    /// A `PoolId` isn't going to be sufficient context; we'll also need a way to resolve and
    /// assert existence of VRF keys. Possibly in another BTreeSet of known VRF keys.
    fn register(&mut self, params: PoolParams, pointer: CertificatePointer, deposit: Lovelace) {
        let pool_id = params.id;
        let _span = trace_span!(
            ledger::validation::CERTIFICATE_POOL_REGISTRATION,
            pool_id = %pool_id
        );
        let _guard = _span.enter();
        self.state.pools.register(params.id, Arc::new((params, pointer, deposit)))
    }

    fn retire(&mut self, pool: PoolId, epoch: Epoch) {
        let _span = trace_span!(
            ledger::validation::CERTIFICATE_POOL_RETIREMENT,
            pool_id = %pool,
            epoch = u64::from(epoch)
        );
        let _guard = _span.enter();
        self.state.pools.unregister(pool, epoch)
    }
}

impl AccountsSlice for DefaultValidationContext {
    /// The block start state (`self.accounts`) with this block's changes (`self.state`) folded in
    fn lookup(&self, credential: &StakeCredential) -> Option<AccountState> {
        // deregistered in block; gone
        if self.state.accounts.unregistered.contains(credential) {
            return None;
        }

        let mut account = match self.state.accounts.registered.get(credential) {
            Some(bind) => match bind.value {
                // fresh in-block registration; supersedes the block start state
                Some(deposit) => AccountState {
                    deposit,
                    pool: bind.left.as_borrowed().to_option(None),
                    drep: bind.right.as_borrowed().to_option(None),
                    rewards: 0,
                },
                // re-binding layered over the block start state
                None => {
                    let base = self.accounts.get(credential)?;
                    AccountState {
                        deposit: base.deposit,
                        pool: bind.left.as_borrowed().to_option(base.pool.as_ref()),
                        drep: bind.right.as_borrowed().to_option(base.drep.as_ref()),
                        rewards: base.rewards,
                    }
                }
            },
            // untouched in block; the block start state
            None => self.accounts.get(credential)?.clone(),
        };

        if self.state.withdrawals.contains(credential) {
            account.rewards = 0;
        }

        Some(account)
    }

    fn register(
        &mut self,
        credential: StakeCredential,
        state: AccountState,
    ) -> Result<(), RegisterError<AccountState, StakeCredential>> {
        let _span =
            trace_span!(ledger::validation::CERTIFICATE_STAKE_REGISTRATION, credential = format!("{credential:?}"));
        let _guard = _span.enter();
        if AccountsSlice::lookup(self, &credential).is_some() {
            return Err(RegisterError::AlreadyRegistered(PhantomData, credential));
        }
        self.state.accounts.register(credential, state.deposit, state.pool, state.drep)?;
        Ok(())
    }

    fn delegate_pool(
        &mut self,
        credential: StakeCredential,
        pool: PoolId,
        pointer: CertificatePointer,
    ) -> Result<(), DelegateError<StakeCredential, PoolId>> {
        let _span = trace_span!(
            ledger::validation::CERTIFICATE_STAKE_DELEGATION,
            credential = format!("{credential:?}"),
            pool_id = %pool
        );
        let _guard = _span.enter();
        if !PoolsSlice::exists(self, pool) {
            return Err(DelegateError::UnknownTarget(pool));
        }
        self.state.accounts.bind_left(credential, Some((pool, pointer)))?;
        Ok(())
    }

    fn delegate_vote(
        &mut self,
        credential: StakeCredential,
        drep: DRep,
        pointer: CertificatePointer,
    ) -> Result<(), DelegateError<StakeCredential, DRep>> {
        let drep_stake_credential: Option<StakeCredential> = match &drep {
            DRep::Key(hash) => Some(StakeCredential::AddrKeyhash(*hash)),
            DRep::Script(hash) => Some(StakeCredential::ScriptHash(*hash)),
            DRep::Abstain | DRep::NoConfidence => None,
        };
        let _span =
            trace_span!(ledger::validation::CERTIFICATE_VOTE_DELEGATION, credential = format!("{credential:?}"));
        if let Some(d) = &drep_stake_credential {
            _span.record("drep", format!("{d:?}"));
        }
        let _guard = _span.enter();
        if let Some(drep_credential) = &drep_stake_credential
            && DRepsSlice::lookup(self, drep_credential).is_none()
        {
            return Err(DelegateError::UnknownTarget(drep));
        }
        self.state.accounts.bind_right(credential, Some((drep, pointer)))?;
        Ok(())
    }

    fn unregister(&mut self, credential: StakeCredential) {
        let _span =
            trace_span!(ledger::validation::CERTIFICATE_STAKE_DEREGISTRATION, credential = format!("{credential:?}"));
        let _guard = _span.enter();
        self.state.accounts.unregister(credential)
    }

    fn withdraw_from(&mut self, credential: StakeCredential) {
        self.state.withdrawals.insert(credential);
    }
}

impl DRepsSlice for DefaultValidationContext {
    fn lookup(&self, credential: &StakeCredential) -> Option<&DRepRegistration> {
        match self.state.dreps.registered.get(credential) {
            // a fresh in-block registration carries its own record; an anchor-only update has no
            // `value`, so fall through to the block-start registration.
            Some(bind) => bind.value.as_deref().or_else(|| self.dreps.get(credential)),
            // deregistered in-block; gone
            None if self.state.dreps.unregistered.contains(credential) => None,
            // untouched in-block; the block-start state
            None => self.dreps.get(credential),
        }
    }

    fn register(
        &mut self,
        drep: StakeCredential,
        registration: DRepRegistration,
        anchor: Option<Anchor>,
    ) -> Result<(), RegisterError<DRepRegistration, StakeCredential>> {
        let _span = trace_span!(
            ledger::validation::CERTIFICATE_DREP_REGISTRATION,
            drep = format!("{drep:?}"),
            deposit = registration.deposit
        );
        if let Some(a) = &anchor {
            _span.record("anchor_url", &a.url);
        }
        let _guard = _span.enter();
        if DRepsSlice::lookup(self, &drep).is_some() {
            return Err(RegisterError::AlreadyRegistered(PhantomData, drep));
        }
        self.state.dreps.register(drep, Arc::new(registration), anchor, None)?;
        Ok(())
    }

    fn update(&mut self, drep: StakeCredential, anchor: Option<Anchor>) -> Result<(), UpdateError<StakeCredential>> {
        let _span = trace_span!(ledger::validation::CERTIFICATE_DREP_UPDATE, drep = format!("{drep:?}"));
        if let Some(a) = &anchor {
            _span.record("anchor_url", &a.url);
        }
        let _guard = _span.enter();
        self.state.dreps.bind_left(drep, anchor)?;
        Ok(())
    }

    fn unregister(&mut self, drep: StakeCredential, refund: Lovelace, pointer: CertificatePointer) {
        let _span =
            trace_span!(ledger::validation::CERTIFICATE_DREP_RETIREMENT, drep = format!("{drep:?}"), refund = refund);
        let _guard = _span.enter();
        self.state.dreps_deregistrations.insert(drep.clone(), pointer);
        self.state.dreps.unregister(drep)
    }
}

impl CommitteeSlice for DefaultValidationContext {
    /// The block start state (`self.committee`) with this block's hot-key change folded in. No
    /// in-block cert establishes membership, so a binding only ever layers over an existing member.
    fn lookup(&self, cc_member: &StakeCredential) -> Option<CCMember> {
        let base = self.committee.get(cc_member);

        match self.state.committee.produced.get(cc_member) {
            Some(hot_credential) => {
                let base = base?;
                Some(CCMember { hot_credential: Some(hot_credential.clone()), valid_until: base.valid_until })
            }
            // resigned in-block; gone
            None if self.state.committee.consumed.contains(cc_member) => None,
            // untouched in-block; the block-start state
            None => base.cloned(),
        }
    }

    fn delegate_cold_key(
        &mut self,
        cc_member: StakeCredential,
        delegate: StakeCredential,
    ) -> Result<(), DelegateError<StakeCredential, StakeCredential>> {
        let _span = trace_span!(
            ledger::validation::CERTIFICATE_COMMITTEE_DELEGATE,
            cc_member = format!("{cc_member:?}"),
            delegate = format!("{delegate:?}")
        );
        let _guard = _span.enter();
        if self.state.committee.consumed.contains(&cc_member) {
            return Err(DelegateError::UnknownSource(cc_member));
        }
        self.state.committee.produce(cc_member, delegate);
        Ok(())
    }

    fn resign(
        &mut self,
        cc_member: StakeCredential,
        anchor: Option<Anchor>,
    ) -> Result<(), UnregisterError<CCMember, StakeCredential>> {
        let _span = trace_span!(ledger::validation::CERTIFICATE_COMMITTEE_RESIGN, cc_member = format!("{cc_member:?}"));
        if let Some(a) = &anchor {
            _span.record("anchor_url", &a.url);
        }
        let _guard = _span.enter();
        self.state.committee.consume(cc_member);
        Ok(())
    }
}

impl ProposalsSlice for DefaultValidationContext {
    fn exists(&self, id: &ComparableProposalId) -> bool {
        // FIXME: also fold proposals discovered in the block during validation
        self.proposals.contains(id)
    }

    fn roots(&self) -> &ProposalsRoots {
        &self.proposals_roots
    }

    fn acknowledge(&mut self, id: ProposalId, pointer: ProposalPointer, proposal: Proposal) {
        self.state.proposals.insert(id.into(), Arc::new((proposal, pointer)));
    }

    fn vote(&mut self, proposal: ProposalId, voter: Voter, vote: Vote, anchor: Option<Anchor>) {
        self.state
            .votes
            .produce(BallotId { proposal: ComparableProposalId::from(proposal), voter }, Ballot::new(vote, anchor))
    }
}

impl WitnessSlice for DefaultValidationContext {
    fn require_vkey_witness(&mut self, vkey_hash: Hash<KEY>) {
        self.required_signers.insert(vkey_hash);
    }

    fn require_script_witness(&mut self, script: RequiredScript) {
        self.required_scripts.insert(script);
    }

    fn acknowledge_script(&mut self, script_hash: Hash<SCRIPT>, location: TransactionInput) {
        self.known_scripts.insert(script_hash, location);
    }

    fn acknowledge_datum(&mut self, datum_hash: Hash<DATUM>, location: TransactionInput) {
        self.known_datums.insert(datum_hash, location);
    }

    fn require_bootstrap_witness(&mut self, root: Hash<28>) {
        self.required_bootstrap_roots.insert(root);
    }

    fn allow_supplemental_datum(&mut self, datum_hash: Hash<DATUM>) {
        self.required_supplemental_datums.insert(datum_hash);
    }

    fn required_signers(&mut self) -> BTreeSet<Hash<KEY>> {
        mem::take(&mut self.required_signers)
    }

    fn required_scripts(&mut self) -> BTreeSet<RequiredScript> {
        mem::take(&mut self.required_scripts)
    }

    fn required_bootstrap_roots(&mut self) -> BTreeSet<Hash<28>> {
        mem::take(&mut self.required_bootstrap_roots)
    }

    fn allowed_supplemental_datums(&mut self) -> BTreeSet<Hash<DATUM>> {
        mem::take(&mut self.required_supplemental_datums)
    }

    fn known_scripts(&mut self) -> BTreeMap<Hash<SCRIPT>, &MemoizedScript> {
        let known_scripts = mem::take(&mut self.known_scripts);
        blanket_known_scripts(self, known_scripts.into_iter())
    }

    fn known_datums(&mut self) -> BTreeMap<Hash<DATUM>, &MemoizedPlutusData> {
        let known_datums = mem::take(&mut self.known_datums);
        blanket_known_datums(self, known_datums.into_iter())
    }
}

impl BalanceSlice for DefaultValidationContext {
    fn consume_value(&mut self, value: &Value) {
        self.balance += value;
    }

    fn produce_value(&mut self, value: &Value) {
        self.balance -= value;
    }

    fn consume_lovelace(&mut self, amount: Lovelace) {
        self.balance += &Value::Coin(amount);
    }

    fn produce_lovelace(&mut self, amount: Lovelace) {
        self.balance -= &Value::Coin(amount);
    }

    fn add_mint(&mut self, mint: &Mint) {
        self.balance += mint;
    }

    fn balance(&mut self) -> Balance {
        mem::take(&mut self.balance)
    }
}

#[cfg(test)]
mod tests {
    use amaru_kernel::{Slot, TransactionPointer};

    use super::*;

    fn cred(tag: u8) -> StakeCredential {
        StakeCredential::AddrKeyhash(Hash::new([tag; 28]))
    }

    fn pointer() -> CertificatePointer {
        CertificatePointer {
            transaction: TransactionPointer { slot: Slot::from(0), transaction_index: 0 },
            certificate_index: 0,
        }
    }

    fn account(rewards: Lovelace) -> AccountState {
        AccountState { deposit: 2_000_000, pool: None, drep: None, rewards }
    }

    fn ctx_with(accounts: BTreeMap<StakeCredential, AccountState>) -> DefaultValidationContext {
        DefaultValidationContext::new(
            BTreeMap::new(),
            BTreeSet::new(),
            accounts,
            BTreeMap::new(),
            BTreeMap::new(),
            BTreeSet::new(),
            ProposalsRoots::default(),
        )
    }

    #[test]
    fn lookup_returns_the_block_start_state_when_untouched() {
        let ctx = ctx_with(BTreeMap::from([(cred(1), account(7))]));
        assert_eq!(AccountsSlice::lookup(&ctx, &cred(1)).map(|a| a.rewards), Some(7));
    }

    #[test]
    fn lookup_reflects_an_in_block_registration() {
        let mut ctx = ctx_with(BTreeMap::new());
        AccountsSlice::register(&mut ctx, cred(1), account(0)).unwrap();
        assert_eq!(AccountsSlice::lookup(&ctx, &cred(1)).map(|a| a.deposit), Some(2_000_000));
    }

    #[test]
    fn lookup_is_none_after_an_in_block_deregistration() {
        let mut ctx = ctx_with(BTreeMap::from([(cred(1), account(7))]));
        AccountsSlice::unregister(&mut ctx, cred(1));
        assert!(AccountsSlice::lookup(&ctx, &cred(1)).is_none());
    }

    #[test]
    fn lookup_zeroes_rewards_after_an_in_block_withdrawal() {
        let mut ctx = ctx_with(BTreeMap::from([(cred(1), account(7))]));
        ctx.withdraw_from(cred(1));
        assert_eq!(AccountsSlice::lookup(&ctx, &cred(1)).map(|a| a.rewards), Some(0));
    }

    #[test]
    fn lookup_layers_an_in_block_delegation_over_the_block_start_state() {
        let pool = Hash::new([9; 28]);
        let mut ctx = DefaultValidationContext::new(
            BTreeMap::new(),
            BTreeSet::from([pool]),
            BTreeMap::from([(cred(1), account(7))]),
            BTreeMap::new(),
            BTreeMap::new(),
            BTreeSet::new(),
            ProposalsRoots::default(),
        );
        ctx.delegate_pool(cred(1), pool, pointer()).unwrap();

        let found = AccountsSlice::lookup(&ctx, &cred(1)).unwrap();
        assert_eq!(found.pool.map(|(p, _)| p), Some(pool));
        assert_eq!(found.rewards, 7);
        assert_eq!(found.deposit, 2_000_000);
    }

    fn cc_member(hot: Option<u8>) -> CCMember {
        CCMember { hot_credential: hot.map(cred), valid_until: Some(Epoch::from(10)) }
    }

    fn ctx_with_committee(committee: BTreeMap<StakeCredential, CCMember>) -> DefaultValidationContext {
        DefaultValidationContext::new(
            BTreeMap::new(),
            BTreeSet::new(),
            BTreeMap::new(),
            BTreeMap::new(),
            committee,
            BTreeSet::new(),
            ProposalsRoots::default(),
        )
    }

    #[test]
    fn committee_lookup_returns_the_block_start_state_when_untouched() {
        let ctx = ctx_with_committee(BTreeMap::from([(cred(1), cc_member(None))]));
        assert_eq!(CommitteeSlice::lookup(&ctx, &cred(1)), Some(cc_member(None)));
    }

    #[test]
    fn committee_lookup_folds_in_an_in_block_hot_key_auth() {
        let mut ctx = ctx_with_committee(BTreeMap::from([(cred(1), cc_member(None))]));
        ctx.delegate_cold_key(cred(1), cred(2)).unwrap();
        assert_eq!(CommitteeSlice::lookup(&ctx, &cred(1)).and_then(|m| m.hot_credential), Some(cred(2)));
    }

    #[test]
    fn committee_lookup_is_none_after_an_in_block_resignation() {
        let mut ctx = ctx_with_committee(BTreeMap::from([(cred(1), cc_member(None))]));
        ctx.resign(cred(1), None).unwrap();
        assert!(CommitteeSlice::lookup(&ctx, &cred(1)).is_none());
    }
}
