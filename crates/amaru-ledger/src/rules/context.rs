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
    Anchor, CertificatePointer, DRep, Lovelace, PoolId, PoolParams, StakeCredential,
    TransactionInput, TransactionOutput,
};

pub mod fake;

/// The BlockValidationContext is a collection of slices needed to validate a block
pub trait BlockValidationContext:
    PotsSlice + UtxoSlice + PoolsSlice + AccountsSlice + DRepsSlice
{
}

/// The BlockPreparationContext is a collection of interfaces needed to prepare a block
pub trait BlockPreparationContext:
    PrepareUtxoSlice + PreparePoolsSlice + PrepareAccountsSlice + PrepareDRepsSlice
{
}

/// An interface for interacting with the protocol pots.
pub trait PotsSlice {
    fn add_fees(&mut self);
}

// An interface for interacting with a subset of the UTxO state.
pub trait UtxoSlice {
    fn lookup(&self, input: &TransactionInput) -> Option<&TransactionOutput>;
    fn consume(&mut self, input: &TransactionInput);
    fn produce(&mut self, input: TransactionInput, output: TransactionOutput);
}

/// An interface to help constructing the concrete UtxoSlice ahead of time.
pub trait PrepareUtxoSlice {
    fn require(&mut self, input: &TransactionInput);
}

/// An interface for interacting with a subset of the Pools state.
pub trait PoolsSlice {
    fn lookup(&self, pool: &PoolId) -> Option<&PoolParams>;
    fn register(&mut self, params: PoolParams);
    fn retire(&mut self, pool: &PoolId);
}

/// An interface to help constructing the concrete PoolsSlice ahead of time.
pub trait PreparePoolsSlice {
    fn require(&mut self, pool: &PoolId);
}

/// An interface for interacting with a subset of the Accounts state.
pub trait AccountsSlice {
    fn lookup(&self, credential: &StakeCredential) -> Option<&AccountState>;
    fn register(&mut self, credential: StakeCredential, state: AccountState);
    fn delegate_pool(&mut self, pool: PoolId);
    fn delegate_vote(&mut self, drep: DRep, ptr: CertificatePointer);
    fn unregister(&mut self, credential: &StakeCredential);
    fn withdraw_from(&mut self, credential: &StakeCredential);
}

/// An interface to help constructing the concrete AccountsSlice ahead of time.
pub trait PrepareAccountsSlice {
    fn require(&mut self, credential: &StakeCredential);
}

#[derive(Debug)]
pub struct AccountState {
    pub deposit: Lovelace,
    pub pool: Option<PoolId>,
    pub drep: Option<(DRep, CertificatePointer)>,
}

/// An interface for interacting with a subset of the DReps state.
pub trait DRepsSlice {
    fn lookup(&self, credential: &DRep) -> Option<&DRepState>;
    fn register(&mut self, drep: DRep, state: DRepState);
    fn update(&mut self, drep: &DRep, anchor: Option<Anchor>);
    fn unregister(&mut self, drep: &DRep);
    fn vote(&mut self, drep: DRep);
}

/// An interface to help constructing the concrete DRepsSlice ahead of time.
pub trait PrepareDRepsSlice {
    fn require(&mut self, credential: &DRep);
}

#[derive(Debug)]
pub struct DRepState {
    pub deposit: Lovelace,
    pub anchor: Option<Anchor>,
    pub registered_at: CertificatePointer,
}
