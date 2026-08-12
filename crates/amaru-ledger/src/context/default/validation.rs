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
    Anchor, Ballot, BallotId, CertificatePointer, ConstitutionalCommitteeMemberStatus, DRep, DRepRegistration, Epoch,
    GovernanceAction, Hash, Lovelace, MemoizedPlutusData, MemoizedScript, MemoizedTransactionOutput, Mint, PoolId,
    PoolParams, ProposalId, ProposalsRoots, RequiredScript, StakeCredential, TransactionInput, Value, Vote, Voter,
    cardano::value::Balance,
    size::{DATUM, KEY, SCRIPT},
};

use crate::{
    context::{
        AccountState, AccountsSlice, BalanceSlice, CCMember, CommitteeSlice, DRepsSlice, DelegateError, PoolVrfs,
        PoolsSlice, PotsSlice, ProposalState, ProposalStateSlim, ProposalsSlice, RegisterError, UnregisterError,
        UpdateError, UtxoSlice, ValidationContext, WitnessSlice, blanket_known_datums, blanket_known_scripts,
    },
    state::volatile::{BindError, Existence, VolatileFragment},
    store::columns::pools_vrf,
};

#[derive(Debug, Default)]
pub struct DefaultValidationContext {
    utxo: BTreeMap<TransactionInput, MemoizedTransactionOutput>,
    pools: BTreeMap<PoolId, PoolVrfs>,
    /// The in-use subset of the block's candidate VRF key hashes, as resolved at the block start.
    vrf_key_hashes_in_use: BTreeSet<pools_vrf::Key>,
    accounts: BTreeMap<StakeCredential, AccountState>,
    dreps: BTreeMap<StakeCredential, DRepRegistration>,
    committee: BTreeMap<StakeCredential, CCMember>,
    proposals: BTreeMap<ProposalId, ProposalStateSlim>,
    proposals_roots: ProposalsRoots,
    treasury: Lovelace,
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
    #[expect(clippy::too_many_arguments)]
    pub fn new(
        utxo: BTreeMap<TransactionInput, MemoizedTransactionOutput>,
        pools: BTreeMap<PoolId, PoolVrfs>,
        vrf_key_hashes_in_use: BTreeSet<pools_vrf::Key>,
        accounts: BTreeMap<StakeCredential, AccountState>,
        dreps: BTreeMap<StakeCredential, DRepRegistration>,
        committee: BTreeMap<StakeCredential, CCMember>,
        proposals: BTreeMap<ProposalId, ProposalStateSlim>,
        proposals_roots: ProposalsRoots,
        treasury: Lovelace,
    ) -> Self {
        Self {
            utxo,
            pools,
            vrf_key_hashes_in_use,
            accounts,
            dreps,
            committee,
            proposals,
            proposals_roots,
            treasury,
            ..Self::default()
        }
    }

    pub fn with_utxo(self, utxo: BTreeMap<TransactionInput, MemoizedTransactionOutput>) -> Self {
        Self { utxo, ..self }
    }

    /// Whether the VRF key hash is occupied at this point in the block: per the in-block claims
    /// and releases first, falling back to the block-start resolution.
    pub fn is_vrf_key_hash_in_use(&self, vrf: &pools_vrf::Key) -> bool {
        match self.state.pools_vrf.get(vrf) {
            Existence::Exists(()) => true,
            Existence::Gone => false,
            Existence::Unknown => self.vrf_key_hashes_in_use.contains(vrf),
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
    fn treasury(&self) -> Lovelace {
        self.treasury
    }

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
        self.pools.contains_key(&pool) || self.state.pools.registered.contains_key(&pool)
    }

    /// Register a pool, mirroring Haskell's split between `psStakePools` and
    /// `psFutureStakePoolParams`: a brand-new pool's parameters (VRF key included) become current
    /// immediately, while a re-registration's only activate at the next epoch boundary, its VRF
    /// key sitting in the pending projection until then.
    fn register(&mut self, params: PoolParams, pointer: CertificatePointer, deposit: Lovelace) {
        if PoolsSlice::exists(self, params.id) {
            self.state.pools_pending_vrf.produce(params.id, params.vrf);
        } else {
            self.state.pools_current_vrf.produce(params.id, params.vrf);
        }

        self.state.pools.register(params.id, Arc::new((params, pointer, deposit)))
    }

    fn retire(&mut self, pool: PoolId, epoch: Epoch) -> Result<(), UnregisterError<PoolId, PoolId>> {
        if !PoolsSlice::exists(self, pool) {
            return Err(UnregisterError::Unknown(PhantomData {}, pool));
        }
        self.state.pools.unregister(pool, epoch);

        Ok(())
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
                    pool: bind.left.as_refs().to_option(None),
                    drep: bind.right.as_refs().to_option(None),
                    rewards: 0,
                },
                // re-binding layered over the block start state
                None => {
                    let base = self.accounts.get(credential)?;
                    AccountState {
                        deposit: base.deposit,
                        pool: bind.left.as_refs().to_option(base.pool.as_ref()),
                        drep: bind.right.as_refs().to_option(base.drep.as_ref()),
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
        if let Some(drep_credential) = &drep_stake_credential
            && DRepsSlice::lookup(self, drep_credential).is_none()
        {
            return Err(DelegateError::UnknownTarget(drep));
        }
        self.state.accounts.bind_right(credential, Some((drep, pointer)))?;
        Ok(())
    }

    fn unregister(&mut self, credential: StakeCredential) {
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
            Some(bind) => bind.value.as_ref().or_else(|| self.dreps.get(credential)),
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
        anchor: Option<Box<Anchor>>,
    ) -> Result<(), RegisterError<DRepRegistration, StakeCredential>> {
        if DRepsSlice::lookup(self, &drep).is_some() {
            return Err(RegisterError::AlreadyRegistered(PhantomData, drep));
        }
        self.state.dreps.register(drep, registration, anchor, None)?;
        Ok(())
    }

    fn update(
        &mut self,
        drep: StakeCredential,
        anchor: Option<Box<Anchor>>,
    ) -> Result<(), UpdateError<StakeCredential>> {
        self.state.dreps.bind_left(drep, anchor)?;
        Ok(())
    }

    fn unregister(&mut self, drep: StakeCredential, _refund: Lovelace, pointer: CertificatePointer) {
        self.state.dreps_deregistrations.insert(drep, pointer);
        self.state.dreps.unregister(drep)
    }
}

impl CommitteeSlice for DefaultValidationContext {
    /// Lookup any known cold credential of a committee member.
    ///
    /// Interestingly, this function may return non-elected committee members that are pending in
    /// proposals, but not yet elected.
    fn lookup_by_cold_credential(&self, cold_credential: &StakeCredential) -> Option<CCMember> {
        match self.state.committee.get(cold_credential) {
            Existence::Gone => None,
            Existence::Exists(new) => {
                let old = self.committee.get(cold_credential);
                Some(CCMember {
                    status: new.left.to_option(old.and_then(|cc_member| cc_member.status.as_ref())),
                    valid_until: new.right.to_option(old.and_then(|cc_member| cc_member.valid_until.as_ref())),
                })
            }
            Existence::Unknown => self.committee.get(cold_credential).copied(),
        }
    }

    /// Lookup CC members, based on a hot credential, including those introduced in the current
    /// block.
    ///
    /// Notably, there is no restriction on the hot credentials arity, so a single hot credential
    /// could be used by many cold credentials. See also [`authorizedHotCommitteeCredentials`][haskell]
    /// from the Haskell codebase.
    ///
    /// Also, the returned members may contain not-yet-elected members (valid_until is None). This
    /// is because pending CC members are allowed to register credentials ahead of being elected;
    /// although the binding is ultimately removed at the epoch boundary if the member is not
    /// elected.
    ///
    /// The length of this set is *NOT* an authoritative count as CCMembers with the same hot key
    /// and expiration collapse into one.
    ///
    /// [haskell]: https://github.com/IntersectMBO/cardano-ledger/blob/0cfbf861cfb456660a7b73281c6fb714a53d40f9/libs/cardano-ledger-core/src/Cardano/Ledger/State/CertState.hs#L335-L337
    fn lookup_by_hot_credential<'iter>(
        &'iter self,
        hot_credential: &'iter StakeCredential,
    ) -> impl Iterator<Item = CCMember> + 'iter {
        use ConstitutionalCommitteeMemberStatus::*;

        std::iter::empty()
            .chain(self.committee.keys())
            .chain(self.state.committee.registered.keys().filter(|k| !self.committee.contains_key(k)))
            .filter_map(move |cc_member| {
                let member = CommitteeSlice::lookup_by_cold_credential(self, cc_member)?;
                match member.status.as_ref() {
                    Some(DelegatedToHotCredential(candidate_credential)) => {
                        (candidate_credential == hot_credential).then_some(member)
                    }
                    None | Some(Resigned) => None,
                }
            })
    }

    fn delegate_cold_key(
        &mut self,
        cold_credential: StakeCredential,
        delegate: StakeCredential,
    ) -> Result<(), DelegateError<StakeCredential, StakeCredential>> {
        let Some(cc_member) = self.lookup_by_cold_credential(&cold_credential) else {
            return Err(DelegateError::UnknownSource(cold_credential));
        };

        if matches!(cc_member.status, Some(ConstitutionalCommitteeMemberStatus::Resigned)) {
            return Err(DelegateError::AlreadyResigned);
        }

        self.state
            .committee
            .bind_left(cold_credential, Some(ConstitutionalCommitteeMemberStatus::DelegatedToHotCredential(delegate)))
            .map_err(|BindError::AlreadyUnregistered(k)| DelegateError::UnknownSource(k))?;
        Ok(())
    }

    fn resign(
        &mut self,
        cold_credential: StakeCredential,
        _anchor: Option<Box<Anchor>>,
    ) -> Result<(), DelegateError<StakeCredential, StakeCredential>> {
        let Some(cc_member) = self.lookup_by_cold_credential(&cold_credential) else {
            return Err(DelegateError::UnknownSource(cold_credential));
        };

        if matches!(cc_member.status, Some(ConstitutionalCommitteeMemberStatus::Resigned)) {
            return Err(DelegateError::AlreadyResigned);
        }

        self.state
            .committee
            .bind_left(cold_credential, Some(ConstitutionalCommitteeMemberStatus::Resigned))
            .map_err(|BindError::AlreadyUnregistered(k)| DelegateError::UnknownSource(k))?;

        Ok(())
    }
}

impl ProposalsSlice for DefaultValidationContext {
    fn lookup(&self, id: &ProposalId) -> Option<ProposalStateSlim> {
        self.proposals
            .get(id)
            .copied()
            .or_else(|| self.state.proposals.get(id).map(|state| ProposalStateSlim::from(state.as_ref())))
    }

    fn roots(&self) -> &ProposalsRoots {
        &self.proposals_roots
    }

    fn acknowledge(&mut self, id: ProposalId, state: ProposalState) {
        // NOTE: Candidate CC Members
        //
        // Members present in pending proposals are seen and accessible for various operations
        // (delegation, resignation). We materialize this as empty binds for that member. They may
        // be overridden by certificates also present in the transaction. And they are removed when
        // the fragment (and proposals) go out of the volatile and the proposals become available to
        // the store anyway.
        if let GovernanceAction::UpdateCommittee(_, _, added, _) = &state.proposal.gov_action {
            for (cold_credential, _) in added.iter() {
                if !matches!(self.state.committee.get(cold_credential), Existence::Exists(..)) {
                    self.state
                        .committee
                        .bind_left(*cold_credential, None)
                        .unwrap_or_else(|_| unreachable!("committee members are never 'unregistered'"));
                }
            }
        }

        self.state.proposals.insert(id, Arc::new(state));
    }

    fn vote(&mut self, proposal: ProposalId, voter: Voter, vote: Vote, anchor: Option<Anchor>) {
        self.state.votes.produce(BallotId { proposal, voter }, Ballot::new(vote, anchor))
    }
}

impl WitnessSlice for DefaultValidationContext {
    fn require_verification_key_witness(&mut self, verification_key_hash: Hash<KEY>) {
        self.required_signers.insert(verification_key_hash);
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
    use test_case::test_case;

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
        DefaultValidationContext { accounts, ..Default::default() }
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
        let mut ctx = DefaultValidationContext {
            pools: BTreeMap::from([(pool, PoolVrfs { current: Hash::new([9; 32]), pending: None })]),
            accounts: BTreeMap::from([(cred(1), account(7))]),
            ..Default::default()
        };
        ctx.delegate_pool(cred(1), pool, pointer()).unwrap();

        let found = AccountsSlice::lookup(&ctx, &cred(1)).unwrap();
        assert_eq!(found.pool.map(|(p, _)| p), Some(pool));
        assert_eq!(found.rewards, 7);
        assert_eq!(found.deposit, 2_000_000);
    }

    fn cc_member(hot: Option<u8>) -> CCMember {
        cc_member_with(hot, Some(10))
    }

    fn cc_member_with(hot: Option<u8>, valid_until: Option<u64>) -> CCMember {
        CCMember { status: hot.map(cred).map(Into::into), valid_until: valid_until.map(Epoch::from) }
    }

    fn ctx_with_committee(committee: BTreeMap<StakeCredential, CCMember>) -> DefaultValidationContext {
        DefaultValidationContext { committee, ..Default::default() }
    }

    #[test]
    fn committee_lookup_returns_the_block_start_state_when_untouched() {
        let ctx = ctx_with_committee(BTreeMap::from([(cred(1), cc_member(None))]));
        assert_eq!(CommitteeSlice::lookup_by_cold_credential(&ctx, &cred(1)), Some(cc_member(None)));
    }

    #[test]
    fn committee_lookup_folds_in_an_in_block_hot_key_auth() {
        let mut ctx = ctx_with_committee(BTreeMap::from([(cred(1), cc_member(None))]));
        ctx.delegate_cold_key(cred(1), cred(2)).unwrap();
        assert_eq!(
            CommitteeSlice::lookup_by_cold_credential(&ctx, &cred(1))
                .and_then(|m| m.status)
                .and_then(|s| s.as_hot_credential().copied()),
            Some(cred(2))
        );
    }

    #[test]
    fn committee_lookup_folds_in_an_auth_from_a_cold_credential_holding_no_seat() {
        let mut ctx = ctx_with_committee(BTreeMap::from([(cred(1), cc_member_with(None, None))]));
        ctx.delegate_cold_key(cred(1), cred(2)).unwrap();
        assert_eq!(
            CommitteeSlice::lookup_by_cold_credential(&ctx, &cred(1)),
            Some(CCMember { status: Some(cred(2).into()), valid_until: None })
        );
    }

    #[test]
    fn committee_lookup_is_some_even_after_an_in_block_resignation() {
        let mut ctx = ctx_with_committee(BTreeMap::from([(cred(1), cc_member(None))]));
        ctx.resign(cred(1), None).unwrap();
        assert!(CommitteeSlice::lookup_by_cold_credential(&ctx, &cred(1)).is_some());
    }

    enum InBlock {
        Nothing,
        Authorize(u8, u8),
        Resign(u8),
    }

    /// A vote identifies its member by hot credential, so the reverse direction has to agree with
    /// `lookup` on everything the ongoing block changes. In particular, a rotation must retire the
    /// previous hot credential rather than leave both live, and an authorization is reachable whether
    /// or not its cold credential held a seat at block start.
    #[test_case(&[(cred(1), cc_member(Some(2)))], InBlock::Nothing, 2, &[cc_member(Some(2))]; "authorized at block start")]
    #[test_case(&[(cred(1), cc_member(Some(2)))], InBlock::Nothing, 3, &[]; "not authorized by anyone")]
    #[test_case(&[(cred(1), cc_member(None))], InBlock::Authorize(1, 2), 2, &[cc_member(Some(2))]; "authorized by this block")]
    #[test_case(&[(cred(1), cc_member(Some(2)))], InBlock::Authorize(1, 3), 3, &[cc_member(Some(3))]; "rotated to in this block")]
    #[test_case(&[(cred(1), cc_member(Some(2)))], InBlock::Authorize(1, 3), 2, &[]; "rotated away from in this block")]
    #[test_case(&[(cred(1), cc_member(Some(2)))], InBlock::Resign(1), 2, &[]; "resigned in this block")]
    #[test_case(
        &[(cred(1), cc_member_with(None, None))],
        InBlock::Authorize(1, 2),
        2,
        &[cc_member_with(Some(2), None)]
        ; "authorized by a cold credential holding no seat"
    )]
    #[test_case(
        &[
            (cred(1), cc_member_with(Some(2), Some(10))),
            (cred(2), cc_member_with(Some(2), Some(20))),
        ],
        InBlock::Nothing,
        2,
        &[
            cc_member_with(Some(2), Some(10)),
            cc_member_with(Some(2), Some(20)),
        ]
        ; "multiple same hot credentials at start"
    )]
    #[test_case(
        &[
            (cred(1), cc_member_with(Some(2), None)),
            (cred(2), cc_member_with(Some(3), None)),
        ],
        InBlock::Authorize(2, 2),
        2,
        &[
            cc_member_with(Some(2), None),
            cc_member_with(Some(2), None),
        ]
        ; "rotate to other cc's hot credential"
    )]
    #[test_case(&[], InBlock::Nothing, 2, &[]; "no members at all")]
    fn committee_lookup_by_hot_credential(
        initial_members: &[(StakeCredential, CCMember)],
        in_block: InBlock,
        queried: u8,
        expected_members: &[CCMember],
    ) {
        let mut ctx = ctx_with_committee(initial_members.iter().copied().collect());

        match in_block {
            InBlock::Nothing => {}
            InBlock::Authorize(cold, hot) => ctx.delegate_cold_key(cred(cold), cred(hot)).unwrap(),
            InBlock::Resign(cold) => ctx.resign(cred(cold), None).unwrap(),
        }

        assert_eq!(
            CommitteeSlice::lookup_by_hot_credential(&ctx, &cred(queried)).collect::<Vec<_>>().as_slice(),
            expected_members,
        );
    }

    /// Nothing stops two seats from authorizing the same hot credential, and nothing resolves that
    /// back to one of them, so both are returned.
    #[test]
    fn committee_lookup_by_hot_credential_returns_every_authorizing_member() {
        let first = CCMember { status: Some(cred(9).into()), valid_until: Some(Epoch::from(10)) };
        let second = CCMember { status: Some(cred(9).into()), valid_until: Some(Epoch::from(20)) };
        let ctx = ctx_with_committee(BTreeMap::from([(cred(1), first), (cred(2), second)]));

        assert_eq!(
            CommitteeSlice::lookup_by_hot_credential(&ctx, &cred(9)).collect::<BTreeSet<_>>(),
            BTreeSet::from([first, second])
        );
    }
}
