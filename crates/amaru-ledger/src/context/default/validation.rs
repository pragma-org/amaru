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
    MemoizedPlutusData, MemoizedScript, MemoizedTransactionOutput, PoolId, PoolParams, Proposal, ProposalId,
    ProposalPointer, RequiredScript, StakeCredential, TransactionInput, Vote, Voter,
    size::{DATUM, KEY, SCRIPT},
};
use amaru_observability::trace_span;

use crate::{
    context::{
        AccountState, AccountsSlice, CCMember, CommitteeSlice, DRepsSlice, DelegateError, PoolsSlice, PotsSlice,
        ProposalsSlice, RegisterError, UnregisterError, UpdateError, UtxoSlice, ValidationContext, WitnessSlice,
        blanket_known_datums, blanket_known_scripts,
    },
    state::{drep_state::DRepState, volatile::VolatileFragment},
};

#[derive(Debug)]
pub struct DefaultValidationContext {
    utxo: BTreeMap<TransactionInput, Arc<MemoizedTransactionOutput>>,
    accounts: BTreeMap<StakeCredential, Arc<AccountState>>,
    dreps: BTreeMap<StakeCredential, Arc<DRepState>>,
    state: VolatileFragment,
    known_scripts: BTreeMap<Hash<SCRIPT>, TransactionInput>,
    known_datums: BTreeMap<Hash<DATUM>, TransactionInput>,
    required_signers: BTreeSet<Hash<KEY>>,
    required_scripts: BTreeSet<RequiredScript>,
    required_supplemental_datums: BTreeSet<Hash<DATUM>>,
    required_bootstrap_roots: BTreeSet<Hash<28>>,
}

impl DefaultValidationContext {
    pub fn new(
        utxo: BTreeMap<TransactionInput, Arc<MemoizedTransactionOutput>>,
        accounts: BTreeMap<StakeCredential, Arc<AccountState>>,
        dreps: BTreeMap<StakeCredential, Arc<DRepState>>,
    ) -> Self {
        Self {
            utxo,
            accounts,
            dreps,
            state: VolatileFragment::default(),
            required_signers: BTreeSet::default(),
            known_scripts: BTreeMap::new(),
            known_datums: BTreeMap::new(),
            required_scripts: BTreeSet::default(),
            required_supplemental_datums: BTreeSet::default(),
            required_bootstrap_roots: BTreeSet::default(),
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
}

impl UtxoSlice for DefaultValidationContext {
    fn lookup(&self, input: &TransactionInput) -> Option<&MemoizedTransactionOutput> {
        self.utxo.get(input).or_else(|| self.state.utxo.produced.get(input)).map(|output| output.as_ref())
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
    fn lookup(&self, _pool: &PoolId) -> Option<&PoolParams> {
        unimplemented!()
    }

    fn register(&mut self, params: PoolParams, pointer: CertificatePointer) {
        let pool_id = params.id;
        let _span = trace_span!(
            amaru_observability::amaru::ledger::context::default::validation::CERTIFICATE_POOL_REGISTRATION,
            pool_id = %pool_id
        );
        let _guard = _span.enter();
        self.state.pools.register(params.id, (params, pointer))
    }

    fn retire(&mut self, pool: PoolId, epoch: Epoch) {
        let _span = trace_span!(
            amaru_observability::amaru::ledger::context::default::validation::CERTIFICATE_POOL_RETIREMENT,
            pool_id = %pool,
            epoch = u64::from(epoch)
        );
        let _guard = _span.enter();
        self.state.pools.unregister(pool, epoch)
    }
}

impl AccountsSlice for DefaultValidationContext {
    fn lookup(&self, credential: &StakeCredential) -> Option<&AccountState> {
        self.state
            .accounts
            .produced
            .get(credential)
            .or_else(|| self.accounts.get(credential))
            .map(|account| account.as_ref())
    }

    fn register(
        &mut self,
        credential: StakeCredential,
        state: AccountState,
    ) -> Result<(), RegisterError<AccountState, StakeCredential>> {
        let _span = trace_span!(
            amaru_observability::amaru::ledger::context::default::validation::CERTIFICATE_STAKE_REGISTRATION,
            credential = format!("{credential:?}")
        );
        let _guard = _span.enter();
        if self.state.accounts.produced.contains_key(&credential) {
            return Err(RegisterError::AlreadyRegistered(PhantomData, credential));
        }
        self.state.accounts.produce(credential, Arc::new(state));
        Ok(())
    }

    fn delegate_pool(
        &mut self,
        credential: StakeCredential,
        pool: PoolId,
        pointer: CertificatePointer,
    ) -> Result<(), DelegateError<StakeCredential, PoolId>> {
        let _span = trace_span!(
            amaru_observability::amaru::ledger::context::default::validation::CERTIFICATE_STAKE_DELEGATION,
            credential = format!("{credential:?}"),
            pool_id = %pool
        );
        let _guard = _span.enter();
        if self.state.accounts.consumed.contains(&credential) {
            return Err(DelegateError::UnknownSource(credential));
        }
        if let Some(current) = AccountsSlice::lookup(self, &credential).cloned() {
            self.state.accounts.produce(credential, Arc::new(AccountState { pool: Some((pool, pointer)), ..current }));
        }
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
        let _span = trace_span!(
            amaru_observability::amaru::ledger::context::default::validation::CERTIFICATE_VOTE_DELEGATION,
            credential = format!("{credential:?}")
        );
        if let Some(d) = &drep_stake_credential {
            _span.record("drep", format!("{d:?}"));
        }
        let _guard = _span.enter();
        if self.state.accounts.consumed.contains(&credential) {
            return Err(DelegateError::UnknownSource(credential));
        }
        if let Some(current) = AccountsSlice::lookup(self, &credential).cloned() {
            self.state.accounts.produce(credential, Arc::new(AccountState { drep: Some((drep, pointer)), ..current }));
        }
        Ok(())
    }

    fn unregister(&mut self, credential: StakeCredential) {
        let _span = trace_span!(
            amaru_observability::amaru::ledger::context::default::validation::CERTIFICATE_STAKE_DEREGISTRATION,
            credential = format!("{credential:?}")
        );
        let _guard = _span.enter();
        self.state.accounts.consume(credential)
    }

    fn withdraw_from(&mut self, credential: StakeCredential) {
        self.state.withdrawals.insert(credential);
    }
}

impl DRepsSlice for DefaultValidationContext {
    fn lookup(&self, credential: &StakeCredential) -> Option<&DRepState> {
        self.state.dreps.produced.get(credential).or_else(|| self.dreps.get(credential)).map(|state| state.as_ref())
    }

    fn register(
        &mut self,
        drep: StakeCredential,
        registration: DRepRegistration,
        anchor: Option<Anchor>,
    ) -> Result<(), RegisterError<DRepRegistration, StakeCredential>> {
        let _span = trace_span!(
            amaru_observability::amaru::ledger::context::default::validation::CERTIFICATE_DREP_REGISTRATION,
            drep = format!("{drep:?}"),
            deposit = registration.deposit
        );
        if let Some(a) = &anchor {
            _span.record("anchor_url", &a.url);
        }
        let _guard = _span.enter();
        if self.state.dreps.produced.contains_key(&drep) {
            return Err(RegisterError::AlreadyRegistered(PhantomData, drep));
        }
        let DRepRegistration { deposit, registered_at, valid_until } = registration;
        self.state.dreps.produce(drep, Arc::new(DRepState { deposit, anchor, registered_at, valid_until }));
        Ok(())
    }

    fn update(
        &mut self,
        drep: StakeCredential,
        anchor: Option<Anchor>,
        valid_until: Epoch,
    ) -> Result<(), UpdateError<StakeCredential>> {
        let _span = trace_span!(
            amaru_observability::amaru::ledger::context::default::validation::CERTIFICATE_DREP_UPDATE,
            drep = format!("{drep:?}")
        );
        if let Some(a) = &anchor {
            _span.record("anchor_url", &a.url);
        }
        let _guard = _span.enter();
        if self.state.dreps.consumed.contains(&drep) {
            return Err(UpdateError::UnknownSource(drep));
        }
        if let Some(current) = DRepsSlice::lookup(self, &drep).cloned() {
            self.state.dreps.produce(drep, Arc::new(DRepState { anchor, valid_until, ..current }));
        }
        Ok(())
    }

    fn unregister(&mut self, drep: StakeCredential, refund: Lovelace) {
        let _span = trace_span!(
            amaru_observability::amaru::ledger::context::default::validation::CERTIFICATE_DREP_RETIREMENT,
            drep = format!("{drep:?}"),
            refund = refund
        );
        let _guard = _span.enter();
        self.state.dreps.consume(drep)
    }
}

impl CommitteeSlice for DefaultValidationContext {
    fn delegate_cold_key(
        &mut self,
        cc_member: StakeCredential,
        delegate: StakeCredential,
    ) -> Result<(), DelegateError<StakeCredential, StakeCredential>> {
        let _span = trace_span!(
            amaru_observability::amaru::ledger::context::default::validation::CERTIFICATE_COMMITTEE_DELEGATE,
            cc_member = format!("{cc_member:?}"),
            delegate = format!("{delegate:?}")
        );
        let _guard = _span.enter();
        self.state.committee.bind_left(cc_member, Some(delegate))?;
        Ok(())
    }

    fn resign(
        &mut self,
        cc_member: StakeCredential,
        anchor: Option<Anchor>,
    ) -> Result<(), UnregisterError<CCMember, StakeCredential>> {
        let _span = trace_span!(
            amaru_observability::amaru::ledger::context::default::validation::CERTIFICATE_COMMITTEE_RESIGN,
            cc_member = format!("{cc_member:?}")
        );
        if let Some(a) = &anchor {
            _span.record("anchor_url", &a.url);
        }
        let _guard = _span.enter();
        self.state.committee.unregister(cc_member);
        Ok(())
    }
}

impl ProposalsSlice for DefaultValidationContext {
    fn acknowledge(&mut self, id: ProposalId, pointer: ProposalPointer, proposal: Proposal) {
        self.state.proposals.insert(id.into(), (proposal, pointer));
    }

    fn vote(&mut self, proposal: ProposalId, voter: Voter, vote: Vote, anchor: Option<Anchor>) {
        self.state.votes.produce(
            BallotId { proposal: ComparableProposalId::from(proposal), voter },
            Arc::new(Ballot::new(vote, anchor)),
        )
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
