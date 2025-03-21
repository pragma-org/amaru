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
use std::collections::BTreeMap;

pub struct FakeBlockPreparationContext {
    pub utxo: BTreeMap<TransactionInput, TransactionOutput>,
}

impl From<FakeBlockPreparationContext> for FakeBlockValidationContext {
    fn from(ctx: FakeBlockPreparationContext) -> FakeBlockValidationContext {
        FakeBlockValidationContext { utxo: ctx.utxo }
    }
}

impl BlockPreparationContext for FakeBlockPreparationContext {}

impl PrepareUtxoSlice for FakeBlockPreparationContext {
    #[allow(clippy::panic)]
    fn require(&mut self, input: &TransactionInput) {
        if !self.utxo.contains_key(input) {
            panic!("unknown required input: {input:?}");
        }
    }
}

impl PreparePoolsSlice for FakeBlockPreparationContext {
    #[allow(clippy::panic)]
    fn require(&mut self, _pool: &PoolId) {
        unimplemented!();
    }
}

impl PrepareAccountsSlice for FakeBlockPreparationContext {
    #[allow(clippy::panic)]
    fn require(&mut self, _credential: &StakeCredential) {
        unimplemented!();
    }
}

impl PrepareDRepsSlice for FakeBlockPreparationContext {
    #[allow(clippy::panic)]
    fn require(&mut self, _drep: &DRep) {
        unimplemented!();
    }
}

#[derive(Debug)]
// TODO: Move into a separate module possibly, or eventually just replace with our _real
// implementation_.
pub struct FakeBlockValidationContext {
    utxo: BTreeMap<TransactionInput, TransactionOutput>,
}

impl FakeBlockValidationContext {
    pub fn new() -> Self {
        Self {
            utxo: BTreeMap::new(),
        }
    }
}

impl BlockValidationContext for FakeBlockValidationContext {}

impl PotsSlice for FakeBlockValidationContext {
    fn add_fees(&mut self) {
        unimplemented!()
    }
}

impl UtxoSlice for FakeBlockValidationContext {
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

impl PoolsSlice for FakeBlockValidationContext {
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

impl AccountsSlice for FakeBlockValidationContext {
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

impl DRepsSlice for FakeBlockValidationContext {
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
