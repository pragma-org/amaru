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

pub use default::*;
pub(crate) mod assert;
mod default;

use amaru_kernel::{
    Anchor, CertificatePointer, DRep, Lovelace, PoolId, PoolParams, StakeCredential,
    TransactionInput, TransactionOutput,
};

/// The ValidationContext is a collection of slices needed to validate a block
pub trait ValidationContext:
    PotsSlice + UtxoSlice + PoolsSlice + AccountsSlice + DRepsSlice
{
}

/// The PreparationContext is a collection of interfaces needed to prepare a block
pub trait PreparationContext<'a>:
    PrepareUtxoSlice<'a> + PreparePoolsSlice<'a> + PrepareAccountsSlice<'a> + PrepareDRepsSlice<'a>
{
}

// Pots
// -------------------------------------------------------------------------------------------------

/// An interface for interacting with the protocol pots.
pub trait PotsSlice {
    fn add_fees(&mut self);
}

// UTxO
// -------------------------------------------------------------------------------------------------

// An interface for interacting with a subset of the UTxO state.
pub trait UtxoSlice {
    fn lookup(&self, input: &TransactionInput) -> Option<&TransactionOutput>;
    fn consume(&mut self, input: &TransactionInput);
    fn produce(&mut self, input: TransactionInput, output: TransactionOutput);
}

/// An interface to help constructing the concrete UtxoSlice ahead of time.
pub trait PrepareUtxoSlice<'a> {
    fn require_input(&'_ mut self, input: &'a TransactionInput);
}

// Pools
// ------------------------------------------------------------------------------------------------

/// An interface for interacting with a subset of the Pools state.
pub trait PoolsSlice {
    fn lookup(&self, pool: &PoolId) -> Option<&PoolParams>;
    fn register(&mut self, params: PoolParams);
    fn retire(&mut self, pool: &PoolId);
}

/// An interface to help constructing the concrete PoolsSlice ahead of time.
pub trait PreparePoolsSlice<'a> {
    fn require_pool(&'a mut self, pool: &'a PoolId);
}

// Accounts
// ------------------------------------------------------------------------------------------------

#[derive(Debug)]
pub struct AccountState {
    pub deposit: Lovelace,
    pub pool: Option<PoolId>,
    pub drep: Option<(DRep, CertificatePointer)>,
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
pub trait PrepareAccountsSlice<'a> {
    fn require_account(&'a mut self, credential: &'a StakeCredential);
}

// DRep
// -------------------------------------------------------------------------------------------------

#[derive(Debug)]
pub struct DRepState {
    pub deposit: Lovelace,
    pub anchor: Option<Anchor>,
    pub registered_at: CertificatePointer,
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
pub trait PrepareDRepsSlice<'a> {
    fn require_drep(&'a mut self, credential: &'a DRep);
}
