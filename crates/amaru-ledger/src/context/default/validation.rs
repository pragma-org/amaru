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

use crate::{
    context::{
        AccountState, AccountsSlice, CCMember, CommitteeSlice, DRepsSlice, DelegateError,
        PoolsSlice, PotsSlice, ProposalsSlice, RegisterError, UnregisterError, UpdateError,
        UtxoSlice, ValidationContext, WitnessSlice, blanket_known_datums, blanket_known_scripts,
    },
    state::volatile_db::VolatileState,
};
use amaru_kernel::{
    Anchor, Ballot, BallotId, CertificatePointer, ComparableProposalId, DRep, DRepRegistration,
    Epoch, Hash, Lovelace, MemoizedPlutusData, MemoizedScript, MemoizedTransactionOutput, PoolId,
    PoolParams, Proposal, ProposalId, ProposalPointer, RequiredScript, StakeCredential,
    TransactionInput, Vote, Voter,
    size::{DATUM, KEY, SCRIPT},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    mem,
};
use tracing::trace;

#[derive(Debug)]
pub struct DefaultValidationContext {
    utxo: BTreeMap<TransactionInput, MemoizedTransactionOutput>,
    state: VolatileState,
    known_scripts: BTreeMap<Hash<SCRIPT>, TransactionInput>,
    known_datums: BTreeMap<Hash<DATUM>, TransactionInput>,
    required_signers: BTreeSet<Hash<KEY>>,
    required_scripts: BTreeSet<RequiredScript>,
    required_supplemental_datums: BTreeSet<Hash<DATUM>>,
    required_bootstrap_roots: BTreeSet<Hash<28>>,
}

impl DefaultValidationContext {
    pub fn new(utxo: BTreeMap<TransactionInput, MemoizedTransactionOutput>) -> Self {
        Self {
            utxo,
            state: VolatileState::default(),
            required_signers: BTreeSet::default(),
            known_scripts: BTreeMap::new(),
            known_datums: BTreeMap::new(),
            required_scripts: BTreeSet::default(),
            required_supplemental_datums: BTreeSet::default(),
            required_bootstrap_roots: BTreeSet::default(),
        }
    }
}

impl From<DefaultValidationContext> for VolatileState {
    fn from(ctx: DefaultValidationContext) -> VolatileState {
        ctx.state
    }
}

impl ValidationContext for DefaultValidationContext {
    type FinalState = VolatileState;
}

impl PotsSlice for DefaultValidationContext {
    fn add_fees(&mut self, fees: Lovelace) {
        self.state.fees += fees;
    }
}

impl UtxoSlice for DefaultValidationContext {
    fn lookup(&self, input: &TransactionInput) -> Option<&MemoizedTransactionOutput> {
        self.utxo.get(input).or(self.state.utxo.produced.get(input))
    }

    fn consume(&mut self, input: TransactionInput) {
        self.utxo.remove(&input);
        self.state.utxo.consume(input)
    }

    fn produce(&mut self, input: TransactionInput, output: MemoizedTransactionOutput) {
        self.state.utxo.produce(input, output)
    }
}

impl PoolsSlice for DefaultValidationContext {
    fn lookup(&self, _pool: &PoolId) -> Option<&PoolParams> {
        unimplemented!()
    }

    fn register(&mut self, params: PoolParams, pointer: CertificatePointer) {
        trace!(target: amaru_observability::CERTIFICATE_TARGET, pool_id = %params.id, "certificate.pool.registration");
        self.state.pools.register(params.id, (params, pointer))
    }

    fn retire(&mut self, pool: PoolId, epoch: Epoch) {
        trace!(target: amaru_observability::CERTIFICATE_TARGET, pool_id = %pool, epoch = %epoch, "certificate.pool.retirement");
        self.state.pools.unregister(pool, epoch)
    }
}

impl AccountsSlice for DefaultValidationContext {
    fn lookup(&self, _credential: &StakeCredential) -> Option<&AccountState> {
        unimplemented!()
    }

    fn register(
        &mut self,
        credential: StakeCredential,
        state: AccountState,
    ) -> Result<(), RegisterError<AccountState, StakeCredential>> {
        trace!(target: amaru_observability::CERTIFICATE_TARGET, ?credential, "certificate.stake.registration");
        self.state
            .accounts
            .register(credential, state.deposit, state.pool, state.drep)?;
        Ok(())
    }

    fn delegate_pool(
        &mut self,
        credential: StakeCredential,
        pool: PoolId,
        pointer: CertificatePointer,
    ) -> Result<(), DelegateError<StakeCredential, PoolId>> {
        trace!(target: amaru_observability::CERTIFICATE_TARGET, ?credential, %pool, "certificate.stake.delegation");
        self.state
            .accounts
            .bind_left(credential, Some((pool, pointer)))?;
        Ok(())
    }

    fn delegate_vote(
        &mut self,
        credential: StakeCredential,
        drep: DRep,
        pointer: CertificatePointer,
    ) -> Result<(), DelegateError<StakeCredential, DRep>> {
        trace!(target: amaru_observability::CERTIFICATE_TARGET, ?credential, ?drep, "certificate.vote.delegation");
        self.state
            .accounts
            .bind_right(credential, Some((drep, pointer)))?;
        Ok(())
    }

    fn unregister(&mut self, credential: StakeCredential) {
        trace!(target: amaru_observability::CERTIFICATE_TARGET, ?credential, "certificate.stake.deregistration");
        self.state.accounts.unregister(credential)
    }

    fn withdraw_from(&mut self, credential: StakeCredential) {
        self.state.withdrawals.insert(credential);
    }
}

impl DRepsSlice for DefaultValidationContext {
    fn lookup(&self, _credential: &StakeCredential) -> Option<&DRepRegistration> {
        unimplemented!()
    }

    fn register(
        &mut self,
        drep: StakeCredential,
        registration: DRepRegistration,
        anchor: Option<Anchor>,
    ) -> Result<(), RegisterError<DRepRegistration, StakeCredential>> {
        trace!(target: amaru_observability::CERTIFICATE_TARGET, ?drep, deposit = %registration.deposit, "certificate.drep.registration");
        self.state
            .dreps
            .register(drep, registration, anchor, None)?;
        Ok(())
    }

    fn update(
        &mut self,
        drep: StakeCredential,
        anchor: Option<Anchor>,
    ) -> Result<(), UpdateError<StakeCredential>> {
        trace!(target: amaru_observability::CERTIFICATE_TARGET, ?drep, ?anchor, "certificate.drep.update");
        self.state.dreps.bind_left(drep, anchor)?;
        Ok(())
    }

    fn unregister(&mut self, drep: StakeCredential, refund: Lovelace, pointer: CertificatePointer) {
        trace!(target: amaru_observability::CERTIFICATE_TARGET, ?drep, ?refund, "certificate.drep.retirement");
        self.state
            .dreps_deregistrations
            .insert(drep.clone(), pointer);
        self.state.dreps.unregister(drep)
    }
}

impl CommitteeSlice for DefaultValidationContext {
    fn delegate_cold_key(
        &mut self,
        cc_member: StakeCredential,
        delegate: StakeCredential,
    ) -> Result<(), DelegateError<StakeCredential, StakeCredential>> {
        trace!(target: amaru_observability::CERTIFICATE_TARGET, ?cc_member, ?delegate, "certificate.committee.delegate");
        self.state.committee.bind_left(cc_member, Some(delegate))?;
        Ok(())
    }

    fn resign(
        &mut self,
        cc_member: StakeCredential,
        anchor: Option<Anchor>,
    ) -> Result<(), UnregisterError<CCMember, StakeCredential>> {
        trace!(target: amaru_observability::CERTIFICATE_TARGET, ?cc_member, ?anchor, "certificate.committee.resign");
        self.state.committee.unregister(cc_member);
        Ok(())
    }
}

impl ProposalsSlice for DefaultValidationContext {
    fn acknowledge(&mut self, id: ProposalId, pointer: ProposalPointer, proposal: Proposal) {
        self.state
            .proposals
            .register(id.into(), (proposal, pointer), None, None)
            .unwrap_or_default(); // Can't happen as by construction key is unique
    }

    fn vote(&mut self, proposal: ProposalId, voter: Voter, vote: Vote, anchor: Option<Anchor>) {
        self.state.votes.produce(
            BallotId {
                proposal: ComparableProposalId::from(proposal),
                voter,
            },
            Ballot::new(vote, anchor),
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
