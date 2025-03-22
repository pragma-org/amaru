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
use crate::rules::{
    context::{
        AccountState, AccountsSlice, DRepState, DRepsSlice, PoolsSlice, PotsSlice,
        PrepareAccountsSlice, PrepareDRepsSlice, PreparePoolsSlice, UtxoSlice,
    },
    BlockPreparationContext, BlockValidationContext, PrepareUtxoSlice,
};
use amaru_kernel::{
    Anchor, CertificatePointer, DRep, PoolId, PoolParams, StakeCredential, TransactionInput,
    TransactionOutput,
};
use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
};

/// An implementation of the block preparation context that's suitable for use in normal operation.
///
/// It is for now incomplete, but we'll use eventually bridge the gap between the validations and
/// the state management so that this fully replaces the current state module and child modules.
///
/// For now, there's still a bit of duplication between the modules.
#[derive(Debug, Default)]
pub struct SimpleBlockPreparationContext<'a> {
    pub utxo: BTreeSet<&'a TransactionInput>,
}

impl SimpleBlockPreparationContext<'_> {
    pub fn new() -> Self {
        Self {
            utxo: BTreeSet::new(),
        }
    }
}

impl<'a> BlockPreparationContext<'a> for SimpleBlockPreparationContext<'a> {}

impl<'a> PrepareUtxoSlice<'a> for SimpleBlockPreparationContext<'a> {
    fn require_input(&'_ mut self, input: &'a TransactionInput) {
        self.utxo.insert(input);
    }
}

impl<'a> PreparePoolsSlice<'a> for SimpleBlockPreparationContext<'a> {
    fn require_pool(&mut self, _pool: &PoolId) {
        unimplemented!();
    }
}

impl<'a> PrepareAccountsSlice<'a> for SimpleBlockPreparationContext<'a> {
    fn require_account(&mut self, _credential: &StakeCredential) {
        unimplemented!();
    }
}

impl<'a> PrepareDRepsSlice<'a> for SimpleBlockPreparationContext<'a> {
    fn require_drep(&mut self, _drep: &DRep) {
        unimplemented!();
    }
}

#[derive(Debug)]
pub struct SimpleBlockValidationContext<'a> {
    utxo: BTreeMap<Cow<'a, TransactionInput>, TransactionOutput>,
}

impl<'a> SimpleBlockValidationContext<'a> {
    pub fn new(utxo: BTreeMap<&'a TransactionInput, TransactionOutput>) -> Self {
        Self {
            utxo: utxo
                .into_iter()
                .map(|(input, output)| (Cow::Borrowed(input), output))
                .collect(),
        }
    }
}

impl BlockValidationContext for SimpleBlockValidationContext<'_> {}

impl PotsSlice for SimpleBlockValidationContext<'_> {
    fn add_fees(&mut self) {
        unimplemented!()
    }
}

impl UtxoSlice for SimpleBlockValidationContext<'_> {
    fn lookup(&self, input: &TransactionInput) -> Option<&TransactionOutput> {
        self.utxo.get(input)
    }

    fn consume(&mut self, input: &TransactionInput) {
        self.utxo.remove(input);
    }

    fn produce(&mut self, input: TransactionInput, output: TransactionOutput) {
        self.utxo.insert(Cow::Owned(input), output);
    }
}

impl PoolsSlice for SimpleBlockValidationContext<'_> {
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

impl AccountsSlice for SimpleBlockValidationContext<'_> {
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

impl DRepsSlice for SimpleBlockValidationContext<'_> {
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
