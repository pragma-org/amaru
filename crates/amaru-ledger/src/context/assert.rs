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

use crate::context::{
    AccountState, AccountsSlice, CCMember, CommitteeSlice, DRepState, DRepsSlice, DelegateError,
    Hash, PoolsSlice, PotsSlice, PreparationContext, PrepareAccountsSlice, PrepareDRepsSlice,
    PreparePoolsSlice, PrepareUtxoSlice, ProposalsSlice, RegisterError, UnregisterError,
    UpdateError, UtxoSlice, ValidationContext, WitnessSlice,
};
use amaru_kernel::{
    Anchor, Bytes, CertificatePointer, DRep, Epoch, Lovelace, PoolId, PoolParams,
    PostAlonzoTransactionOutput, Proposal, ProposalPointer, StakeCredential, TransactionInput,
    TransactionOutput, Value,
};
use core::mem;
use std::collections::{BTreeMap, BTreeSet};

// ------------------------------------------------------------------------------------- Preparation

/// A Fake block preparation context that can used for testing. The context is expected to be
/// provided upfront as test data, and all `require` method merely checks that the requested data
/// pre-exists in the context.
#[derive(Debug, Clone)]
pub struct AssertPreparationContext {
    pub utxo: BTreeMap<TransactionInput, TransactionOutput>,
}

impl From<AssertPreparationContext> for AssertValidationContext {
    fn from(ctx: AssertPreparationContext) -> AssertValidationContext {
        AssertValidationContext {
            utxo: ctx.utxo,
            required_signers: BTreeSet::default(),
            required_bootstrap_signers: BTreeSet::default(),
        }
    }
}

impl PreparationContext<'_> for AssertPreparationContext {}

impl PrepareUtxoSlice<'_> for AssertPreparationContext {
    #[allow(clippy::panic)]
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
}

impl PrepareDRepsSlice<'_> for AssertPreparationContext {
    fn require_drep(&mut self, _drep: &StakeCredential) {
        unimplemented!();
    }
}

//  Deserialization of Prepreation Context JSON files

#[derive(serde::Deserialize, Debug)]
struct JsonUtxoEntry {
    input: JsonInput,
    output: JsonOutput,
}

#[derive(serde::Deserialize, Debug)]
struct JsonInput {
    transaction_id: String,
    index: u64,
}
#[derive(serde::Deserialize, Debug)]
struct JsonOutput {
    #[serde(deserialize_with = "deserialize_hex_to_vec")]
    address: Vec<u8>,
    // TODO: expand this
}

// TODO: Should this live somewhere else?
fn deserialize_hex_to_vec<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s: String = serde::Deserialize::deserialize(deserializer)?;
    hex::decode(s).map_err(serde::de::Error::custom)
}

impl From<JsonOutput> for TransactionOutput {
    fn from(json: JsonOutput) -> TransactionOutput {
        TransactionOutput::PostAlonzo(PostAlonzoTransactionOutput {
            address: Bytes::from(json.address),
            value: Value::Coin(0),
            datum_option: None,
            script_ref: None,
        })
    }
}

impl TryFrom<JsonInput> for TransactionInput {
    type Error = hex::FromHexError;

    fn try_from(value: JsonInput) -> Result<Self, Self::Error> {
        Ok(TransactionInput {
            transaction_id: value.transaction_id.parse()?,
            index: value.index,
        })
    }
}

impl<'de> serde::Deserialize<'de> for AssertPreparationContext {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error;

        let mut map: serde_json::Map<String, serde_json::Value> =
            serde::Deserialize::deserialize(deserializer)?;

        let utxo_value = map
            .remove("utxo_slice")
            .ok_or_else(|| D::Error::custom("missing field `utxo_slice`"))?;

        let entries: Vec<JsonUtxoEntry> = serde_json::from_value(utxo_value)
            .map_err(|e| D::Error::custom(format!("Failed to parse utxo_slice: {}", e)))?;

        let mut utxo = BTreeMap::new();

        for entry in entries {
            utxo.insert(
                entry
                    .input
                    .try_into()
                    .map_err(|_| D::Error::custom("invalid transaction id"))?,
                entry.output.into(),
            );
        }

        Ok(AssertPreparationContext { utxo })
    }
}

// -------------------------------------------------------------------------------------- Validation

#[derive(Debug)]
pub struct AssertValidationContext {
    utxo: BTreeMap<TransactionInput, TransactionOutput>,
    required_signers: BTreeSet<Hash<28>>,
    required_bootstrap_signers: BTreeSet<Hash<28>>,
}

impl ValidationContext for AssertValidationContext {
    type FinalState = ();
}

impl From<AssertValidationContext> for () {
    fn from(_ctx: AssertValidationContext) {}
}

impl PotsSlice for AssertValidationContext {
    fn add_fees(&mut self, _fees: Lovelace) {}
}

impl UtxoSlice for AssertValidationContext {
    fn lookup(&self, input: &TransactionInput) -> Option<&TransactionOutput> {
        self.utxo.get(input)
    }

    fn consume(&mut self, input: TransactionInput) {
        self.utxo.remove(&input);
    }

    fn produce(&mut self, input: TransactionInput, output: TransactionOutput) {
        self.utxo.insert(input, output);
    }
}

impl PoolsSlice for AssertValidationContext {
    fn lookup(&self, _pool: &PoolId) -> Option<&PoolParams> {
        unimplemented!()
    }
    fn register(&mut self, _params: PoolParams) {
        unimplemented!()
    }
    fn retire(&mut self, _pool: PoolId, _epoch: Epoch) {
        unimplemented!()
    }
}

impl AccountsSlice for AssertValidationContext {
    fn lookup(&self, _credential: &StakeCredential) -> Option<&AccountState> {
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

    fn withdraw_from(&mut self, _credential: StakeCredential) {
        unimplemented!()
    }
}

impl DRepsSlice for AssertValidationContext {
    fn lookup(&self, _credential: &StakeCredential) -> Option<&DRepState> {
        unimplemented!()
    }

    fn register(
        &mut self,
        _drep: StakeCredential,
        _state: DRepState,
    ) -> Result<(), RegisterError<DRepState, StakeCredential>> {
        unimplemented!()
    }

    fn update(
        &mut self,
        _drep: StakeCredential,
        _anchor: Option<Anchor>,
    ) -> Result<(), UpdateError<StakeCredential>> {
        unimplemented!()
    }

    fn unregister(&mut self, _drep: StakeCredential, _refund: Lovelace) {
        unimplemented!()
    }

    fn vote(&mut self, _drep: StakeCredential) {
        unimplemented!()
    }
}

impl CommitteeSlice for AssertValidationContext {
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
    fn acknowledge(&mut self, _pointer: ProposalPointer, _proposal: Proposal) {}
}

impl WitnessSlice for AssertValidationContext {
    fn require_witness(&mut self, credential: StakeCredential) {
        match credential {
            StakeCredential::AddrKeyhash(vk_hash) => {
                self.required_signers.insert(vk_hash);
            }
            StakeCredential::ScriptHash(..) => {
                // FIXME: Also account for native scripts. We should pre-fetch necessary scripts
                // before hand, and here, check whether additional signatures are needed.
            }
        }
    }

    fn require_bootstrap_witness(&mut self, root: Hash<28>) {
        self.required_bootstrap_signers.insert(root);
    }

    fn required_signers(&mut self) -> BTreeSet<Hash<28>> {
        mem::take(&mut self.required_signers)
    }

    fn required_bootstrap_signers(&mut self) -> BTreeSet<Hash<28>> {
        mem::take(&mut self.required_bootstrap_signers)
    }
}
