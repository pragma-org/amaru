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
    mem,
};

use amaru_kernel::{
    Anchor, AsHash, CertificatePointer, ComparableProposalId, DRep, DRepRegistration, Epoch, Hash, Lovelace,
    MemoizedPlutusData, MemoizedScript, MemoizedTransactionOutput, Mint, PoolId, PoolParams, Proposal, ProposalId,
    ProposalPointer, RequiredScript, RewardAccount, StakeCredential, StakeCredentialKind, TransactionInput, Value,
    Vote, Voter, VoterKind,
    cardano::value::Balance,
    size::{DATUM, KEY, SCRIPT},
    utils::serde::deserialize_map_proxy,
};
use tracing::instrument;

use crate::{
    context::{
        AccountState, AccountsSlice, BalanceSlice, CCMember, CommitteeSlice, DRepsSlice, DelegateError, PoolsSlice,
        PotsSlice, PreparationContext, PrepareAccountsSlice, PrepareCommitteeSlice, PrepareDRepsSlice,
        PreparePoolsSlice, PrepareProposalsSlice, PrepareUtxoSlice, ProposalsSlice, RegisterError, UnregisterError,
        UpdateError, UtxoSlice, ValidationContext, WitnessSlice, blanket_known_datums, blanket_known_scripts,
    },
    governance::ratification::ProposalsRoots,
};

// ------------------------------------------------------------------------------------- Preparation

/// A Fake block preparation context that can used for testing. The context is expected to be
/// provided upfront as test data, and all `require` method merely checks that the requested data
/// pre-exists in the context.
#[derive(serde::Deserialize, Debug, Clone)]
pub struct AssertPreparationContext {
    #[serde(deserialize_with = "deserialize_map_proxy")]
    pub utxo: BTreeMap<TransactionInput, MemoizedTransactionOutput>,
}

impl From<AssertPreparationContext> for AssertValidationContext {
    fn from(ctx: AssertPreparationContext) -> AssertValidationContext {
        AssertValidationContext {
            utxo: ctx.utxo,
            required_signers: BTreeSet::default(),
            required_scripts: BTreeSet::default(),
            known_scripts: BTreeMap::default(),
            known_datums: BTreeMap::default(),
            required_supplemental_datums: BTreeSet::default(),
            required_bootstrap_roots: BTreeSet::default(),
            balance: Balance::default(),
        }
    }
}

impl PreparationContext<'_> for AssertPreparationContext {}

impl PrepareUtxoSlice<'_> for AssertPreparationContext {
    #[expect(clippy::panic)]
    fn require_input(&mut self, input: &TransactionInput) {
        if !self.utxo.contains_key(input) {
            panic!("unknown required input: {input:?}");
        }
    }
}

impl PreparePoolsSlice<'_> for AssertPreparationContext {
    fn require_pool(&mut self, _pool: &PoolId) {
        unimplemented!();
    }
}

impl PrepareAccountsSlice<'_> for AssertPreparationContext {
    fn require_account(&mut self, _credential: &StakeCredential) {
        unimplemented!();
    }

    fn require_withdrawal(&mut self, _reward_account: &RewardAccount) {
        unimplemented!();
    }
}

impl PrepareDRepsSlice<'_> for AssertPreparationContext {
    fn require_drep(&mut self, _drep: &StakeCredential) {
        unimplemented!();
    }

    fn require_drep_delegation(&mut self, _drep: &DRep) {
        unimplemented!();
    }
}

impl PrepareCommitteeSlice<'_> for AssertPreparationContext {
    fn require_committee_member(&mut self, _cc_member: &StakeCredential) {
        unimplemented!();
    }
}

impl PrepareProposalsSlice<'_> for AssertPreparationContext {
    fn require_proposal(&mut self, _id: &ProposalId) {
        unimplemented!();
    }
}

// -------------------------------------------------------------------------------------- Validation

#[derive(Debug, serde::Deserialize)]
pub struct AssertValidationContext {
    #[serde(deserialize_with = "deserialize_map_proxy")]
    utxo: BTreeMap<TransactionInput, MemoizedTransactionOutput>,
    #[serde(default)]
    required_signers: BTreeSet<Hash<KEY>>,
    #[serde(default)]
    required_scripts: BTreeSet<RequiredScript>,
    #[serde(default)]
    known_scripts: BTreeMap<Hash<SCRIPT>, TransactionInput>,
    #[serde(default)]
    known_datums: BTreeMap<Hash<DATUM>, TransactionInput>,
    #[serde(default)]
    required_supplemental_datums: BTreeSet<Hash<DATUM>>,
    #[serde(default)]
    required_bootstrap_roots: BTreeSet<Hash<28>>,
    #[serde(default)]
    balance: Balance,
}

impl ValidationContext for AssertValidationContext {
    type FinalState = ();
}

impl From<AssertValidationContext> for () {
    fn from(_ctx: AssertValidationContext) {}
}

impl PotsSlice for AssertValidationContext {
    #[instrument(skip_all, fields(fee = fee))]
    fn add_fees(&mut self, fee: Lovelace) {}

    fn add_donation(&mut self, _donation: Lovelace) {
        unimplemented!()
    }
}

impl UtxoSlice for AssertValidationContext {
    fn lookup(&self, input: &TransactionInput) -> Option<&MemoizedTransactionOutput> {
        self.utxo.get(input)
    }

    fn consume(&mut self, input: TransactionInput) {
        self.utxo.remove(&input);
    }

    fn produce(&mut self, input: TransactionInput, output: MemoizedTransactionOutput) {
        self.utxo.insert(input, output);
    }
}

impl PoolsSlice for AssertValidationContext {
    fn exists(&self, _pool: PoolId) -> bool {
        unimplemented!()
    }

    fn register(&mut self, _params: PoolParams, _pointer: CertificatePointer, _deposit: Lovelace) {
        unimplemented!()
    }

    fn retire(&mut self, _pool: PoolId, _epoch: Epoch) -> Result<(), UnregisterError<PoolId, PoolId>> {
        unimplemented!()
    }
}

impl AccountsSlice for AssertValidationContext {
    fn lookup(&self, _credential: &StakeCredential) -> Option<AccountState> {
        unimplemented!()
    }

    fn register(
        &mut self,
        _credential: StakeCredential,
        _state: AccountState,
    ) -> Result<(), RegisterError<AccountState, StakeCredential>> {
        unimplemented!()
    }

    fn delegate_pool(
        &mut self,
        _credential: StakeCredential,
        _pool: PoolId,
        _pointer: CertificatePointer,
    ) -> Result<(), DelegateError<StakeCredential, PoolId>> {
        unimplemented!()
    }

    fn delegate_vote(
        &mut self,
        _credential: StakeCredential,
        _drep: DRep,
        _pointer: CertificatePointer,
    ) -> Result<(), DelegateError<StakeCredential, DRep>> {
        unimplemented!()
    }

    fn unregister(&mut self, _credential: StakeCredential) {
        unimplemented!()
    }

    #[instrument(
        skip_all,
        fields(
            credential_type = %StakeCredentialKind::from(&credential),
            credential_hash = %credential.as_hash(),
        ),
    )]
    fn withdraw_from(&mut self, credential: StakeCredential) {}
}

impl DRepsSlice for AssertValidationContext {
    fn lookup(&self, _credential: &StakeCredential) -> Option<&DRepRegistration> {
        unimplemented!()
    }

    fn register(
        &mut self,
        _drep: StakeCredential,
        _registration: DRepRegistration,
        _anchor: Option<Anchor>,
    ) -> Result<(), RegisterError<DRepRegistration, StakeCredential>> {
        unimplemented!()
    }

    fn update(&mut self, _drep: StakeCredential, _anchor: Option<Anchor>) -> Result<(), UpdateError<StakeCredential>> {
        unimplemented!()
    }

    fn unregister(&mut self, _drep: StakeCredential, _refund: Lovelace, _pointer: CertificatePointer) {
        unimplemented!()
    }
}

impl CommitteeSlice for AssertValidationContext {
    fn lookup(&self, _credential: &StakeCredential) -> Option<CCMember> {
        unimplemented!()
    }

    fn delegate_cold_key(
        &mut self,
        _cc_member: StakeCredential,
        _delegate: StakeCredential,
    ) -> Result<(), DelegateError<StakeCredential, StakeCredential>> {
        unimplemented!()
    }

    fn resign(
        &mut self,
        _cc_member: StakeCredential,
        _anchor: Option<Anchor>,
    ) -> Result<(), UnregisterError<CCMember, StakeCredential>> {
        unimplemented!()
    }
}

impl ProposalsSlice for AssertValidationContext {
    fn exists(&self, _id: &ComparableProposalId) -> bool {
        unimplemented!()
    }

    fn roots(&self) -> &ProposalsRoots {
        unimplemented!()
    }

    fn acknowledge(&mut self, _id: ProposalId, _pointer: ProposalPointer, _proposal: Proposal) {}

    #[instrument(
        skip_all,
        fields(
            voter_type = %VoterKind::from(&voter),
            credential_type = %StakeCredentialKind::from(&voter),
            credential_hash = %voter.as_hash(),
        ),
    )]
    fn vote(&mut self, _proposal: ProposalId, voter: Voter, _vote: Vote, _anchor: Option<Anchor>) {}
}

impl WitnessSlice for AssertValidationContext {
    #[instrument(skip_all, fields(hash = %vkey_hash))]
    fn require_vkey_witness(&mut self, vkey_hash: Hash<28>) {
        self.required_signers.insert(vkey_hash);
    }

    #[instrument(skip_all, fields(hash = %script.hash))]
    fn require_script_witness(&mut self, script: RequiredScript) {
        self.required_scripts.insert(script);
    }

    fn acknowledge_script(&mut self, script_hash: Hash<SCRIPT>, location: TransactionInput) {
        self.known_scripts.insert(script_hash, location);
    }

    fn acknowledge_datum(&mut self, datum_hash: Hash<DATUM>, location: TransactionInput) {
        self.known_datums.insert(datum_hash, location);
    }

    #[instrument(skip_all, fields(bootstrap_witness_hash = %root))]
    fn require_bootstrap_witness(&mut self, root: Hash<28>) {
        self.required_bootstrap_roots.insert(root);
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

    fn allow_supplemental_datum(&mut self, datum_hash: Hash<DATUM>) {
        self.required_supplemental_datums.insert(datum_hash);
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

impl BalanceSlice for AssertValidationContext {
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
