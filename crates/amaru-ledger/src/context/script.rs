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

use amaru_kernel::{
    Anchor, CertificatePointer, DRep, DRepRegistration, DatumHash, Hash, Lovelace,
    MemoizedPlutusData, MemoizedScript, MemoizedTransactionOutput, PoolId, PoolParams, Proposal,
    ProposalId, ProposalPointer, RequiredScript, ScriptHash, StakeCredential, TransactionInput,
    Vote, Voter,
    arc_mapped::ArcMapped,
    context::{
        AccountState, AccountsSlice, CCMember, CommitteeSlice, DRepsSlice, DelegateError,
        PoolsSlice, PotsSlice, ProposalsSlice, RegisterError, UnregisterError, UpdateError,
        UtxoSlice, ValidationContext, WitnessSlice,
    },
};
use amaru_slot_arithmetic::Epoch;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use amaru_plutus::unstable::TxInfoStorage;
use tracing::error;

use crate::state::VolatileState;

/// A validation context that also constructs a phase-2 script context. This allows to minimize
/// cloning and traversal of transaction constituents during validation. It is built as a wrapper
/// on top of an existing validation context to allow for a clearer separation of concerns.
#[derive(Debug)]
pub struct ScriptEvaluationContext<V: ValidationContext> {
    phase1: V,
    phase2: TxInfoStorage,
}

impl<V: ValidationContext> ScriptEvaluationContext<V> {
    pub fn new(phase1: V) -> Self {
        Self {
            phase1,
            phase2: TxInfoStorage::default(),
        }
    }
}

impl<V: ValidationContext + Into<VolatileState>> From<ScriptEvaluationContext<V>>
    for VolatileState
{
    fn from(ctx: ScriptEvaluationContext<V>) -> Self {
        ctx.phase1.into()
    }
}

impl<V: ValidationContext> ValidationContext for ScriptEvaluationContext<V> {
    type FinalState = V::FinalState;
}

impl<V: ValidationContext> PotsSlice for ScriptEvaluationContext<V> {
    fn add_fees(&mut self, fees: Lovelace) {
        self.phase1.add_fees(fees)
    }
}

impl<V: ValidationContext> UtxoSlice for ScriptEvaluationContext<V> {
    fn lookup(&self, input: &TransactionInput) -> Option<&MemoizedTransactionOutput> {
        UtxoSlice::lookup(&self.phase1, input)
    }

    fn get(&self, input: &TransactionInput) -> Option<Arc<MemoizedTransactionOutput>> {
        UtxoSlice::get(&self.phase1, input)
    }

    fn consume(
        &mut self,
        input: TransactionInput,
    ) -> Option<(&TransactionInput, Arc<MemoizedTransactionOutput>)> {
        if let Some((input, output)) = self.phase1.consume(input) {
            self.phase2.add_input(input.clone(), output.clone());
            return Some((input, output));
        }

        error!("invariant violation: UtxoSlice::consume did not consume anything...");

        None
    }

    fn produce(
        &mut self,
        input: TransactionInput,
        output: MemoizedTransactionOutput,
    ) -> Arc<MemoizedTransactionOutput> {
        let output = self.phase1.produce(input, output);
        self.phase2.add_output(output.clone());
        output
    }
}

impl<V: ValidationContext> PoolsSlice for ScriptEvaluationContext<V> {
    fn lookup(&self, pool: &PoolId) -> Option<&PoolParams> {
        PoolsSlice::lookup(&self.phase1, pool)
    }

    fn register(&mut self, params: PoolParams, pointer: CertificatePointer) {
        PoolsSlice::register(&mut self.phase1, params, pointer)
    }

    fn retire(&mut self, pool: PoolId, epoch: Epoch) {
        self.phase1.retire(pool, epoch)
    }
}

impl<V: ValidationContext> AccountsSlice for ScriptEvaluationContext<V> {
    fn lookup(&self, credential: &StakeCredential) -> Option<&AccountState> {
        AccountsSlice::lookup(&self.phase1, credential)
    }

    fn register(
        &mut self,
        credential: StakeCredential,
        state: AccountState,
    ) -> Result<(), RegisterError<AccountState, StakeCredential>> {
        AccountsSlice::register(&mut self.phase1, credential, state)
    }

    fn delegate_pool(
        &mut self,
        credential: StakeCredential,
        pool: PoolId,
        pointer: CertificatePointer,
    ) -> Result<(), DelegateError<StakeCredential, PoolId>> {
        self.phase1.delegate_pool(credential, pool, pointer)
    }

    fn delegate_vote(
        &mut self,
        credential: StakeCredential,
        drep: DRep,
        pointer: CertificatePointer,
    ) -> Result<(), DelegateError<StakeCredential, DRep>> {
        self.phase1.delegate_vote(credential, drep, pointer)
    }

    fn unregister(&mut self, credential: StakeCredential) {
        AccountsSlice::unregister(&mut self.phase1, credential)
    }

    fn withdraw_from(&mut self, credential: StakeCredential) {
        self.phase1.withdraw_from(credential)
    }
}

impl<V: ValidationContext> DRepsSlice for ScriptEvaluationContext<V> {
    fn lookup(&self, credential: &StakeCredential) -> Option<&DRepRegistration> {
        DRepsSlice::lookup(&self.phase1, credential)
    }

    fn register(
        &mut self,
        drep: StakeCredential,
        registration: DRepRegistration,
        anchor: Option<Anchor>,
    ) -> Result<(), RegisterError<DRepRegistration, StakeCredential>> {
        DRepsSlice::register(&mut self.phase1, drep, registration, anchor)
    }

    fn update(
        &mut self,
        drep: StakeCredential,
        anchor: Option<Anchor>,
    ) -> Result<(), UpdateError<StakeCredential>> {
        DRepsSlice::update(&mut self.phase1, drep, anchor)
    }

    fn unregister(&mut self, drep: StakeCredential, refund: Lovelace, pointer: CertificatePointer) {
        DRepsSlice::unregister(&mut self.phase1, drep, refund, pointer)
    }
}

impl<V: ValidationContext> CommitteeSlice for ScriptEvaluationContext<V> {
    fn delegate_cold_key(
        &mut self,
        cc_member: StakeCredential,
        delegate: StakeCredential,
    ) -> Result<(), DelegateError<StakeCredential, StakeCredential>> {
        self.phase1.delegate_cold_key(cc_member, delegate)
    }

    fn resign(
        &mut self,
        cc_member: StakeCredential,
        anchor: Option<Anchor>,
    ) -> Result<(), UnregisterError<CCMember, StakeCredential>> {
        self.phase1.resign(cc_member, anchor)
    }
}

impl<V: ValidationContext> ProposalsSlice for ScriptEvaluationContext<V> {
    fn acknowledge(&mut self, id: ProposalId, pointer: ProposalPointer, proposal: Proposal) {
        self.phase1.acknowledge(id, pointer, proposal)
    }

    fn vote(&mut self, proposal: ProposalId, voter: Voter, vote: Vote, anchor: Option<Anchor>) {
        self.phase1.vote(proposal, voter, vote, anchor)
    }
}

impl<V: ValidationContext> WitnessSlice for ScriptEvaluationContext<V> {
    fn require_vkey_witness(&mut self, vkey_hash: amaru_kernel::AddrKeyhash) {
        self.phase1.require_vkey_witness(vkey_hash)
    }

    fn require_script_witness(&mut self, script: RequiredScript) {
        self.phase1.require_script_witness(script)
    }

    fn acknowledge_script(&mut self, script_hash: ScriptHash, location: TransactionInput) {
        self.phase1.acknowledge_script(script_hash, location)
    }

    fn acknowledge_datum(&mut self, datum_hash: DatumHash, location: TransactionInput) {
        self.phase1.acknowledge_datum(datum_hash, location)
    }

    fn require_bootstrap_witness(&mut self, root: Hash<28>) {
        self.phase1.require_bootstrap_witness(root)
    }

    fn allow_supplemental_datum(&mut self, datum_hash: Hash<32>) {
        self.phase1.allow_supplemental_datum(datum_hash)
    }

    fn required_signers(&mut self) -> BTreeSet<Hash<28>> {
        self.phase1.required_signers()
    }

    fn required_scripts(&mut self) -> BTreeSet<RequiredScript> {
        self.phase1.required_scripts()
    }

    fn required_bootstrap_roots(&mut self) -> BTreeSet<Hash<28>> {
        self.phase1.required_bootstrap_roots()
    }

    fn allowed_supplemental_datums(&mut self) -> BTreeSet<Hash<32>> {
        self.phase1.allowed_supplemental_datums()
    }

    fn known_scripts(&mut self) -> BTreeMap<ScriptHash, &MemoizedScript> {
        self.phase1.known_scripts()
    }

    fn known_datums(
        &mut self,
    ) -> BTreeMap<DatumHash, ArcMapped<MemoizedTransactionOutput, MemoizedPlutusData>> {
        self.phase2.set_datums(self.phase1.known_datums());
        self.phase2.datums().clone()
    }
}
