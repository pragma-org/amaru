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
    AccountState, AccountsSlice, DRepState, DRepsSlice, PoolsSlice, PotsSlice, PreparationContext,
    PrepareAccountsSlice, PrepareDRepsSlice, PreparePoolsSlice, PrepareUtxoSlice, UtxoSlice,
    ValidationContext,
};
use amaru_kernel::{
    Anchor, CertificatePointer, DRep, PoolId, PoolParams, StakeCredential, TransactionInput,
    TransactionOutput,
};
use std::collections::BTreeMap;

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
        AssertValidationContext { utxo: ctx.utxo }
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
    fn require_drep(&mut self, _drep: &DRep) {
        unimplemented!();
    }
}

// -------------------------------------------------------------------------------------- Validation

#[derive(Debug)]
pub struct AssertValidationContext {
    utxo: BTreeMap<TransactionInput, TransactionOutput>,
}

impl ValidationContext for AssertValidationContext {}

impl PotsSlice for AssertValidationContext {
    fn add_fees(&mut self) {
        unimplemented!()
    }
}

impl UtxoSlice for AssertValidationContext {
    fn lookup(&self, input: &TransactionInput) -> Option<&TransactionOutput> {
        self.utxo.get(input)
    }

    fn consume(&mut self, input: &TransactionInput) {
        self.utxo.remove(input);
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
    fn retire(&mut self, _pool: &PoolId) {
        unimplemented!()
    }
}

impl AccountsSlice for AssertValidationContext {
    fn lookup(&self, _credential: &StakeCredential) -> Option<&AccountState> {
        unimplemented!()
    }

    fn register(&mut self, _credential: StakeCredential, _state: AccountState) {
        unimplemented!()
    }

    fn delegate_pool(&mut self, _pool: PoolId) {
        unimplemented!()
    }

    fn delegate_vote(&mut self, _drep: DRep, _ptr: CertificatePointer) {
        unimplemented!()
    }

    fn unregister(&mut self, _credential: &StakeCredential) {
        unimplemented!()
    }

    fn withdraw_from(&mut self, _credential: &StakeCredential) {
        unimplemented!()
    }
}

impl DRepsSlice for AssertValidationContext {
    fn lookup(&self, _credential: &DRep) -> Option<&DRepState> {
        unimplemented!()
    }
    fn register(&mut self, _drep: DRep, _state: DRepState) {
        unimplemented!()
    }
    fn update(&mut self, _drep: &DRep, _anchor: Option<Anchor>) {
        unimplemented!()
    }
    fn unregister(&mut self, _drep: &DRep) {
        unimplemented!()
    }
    fn vote(&mut self, _drep: DRep) {
        unimplemented!()
    }
}
