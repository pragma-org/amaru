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

pub(crate) mod assert;
mod default;

use crate::state::diff_bind;
use amaru_kernel::{
    Anchor, CertificatePointer, DRep, Epoch, Hash, Lovelace, PoolId, PoolParams, Proposal,
    ProposalId, ProposalPointer, StakeCredential, TransactionInput, TransactionOutput,
};
use std::{collections::BTreeSet, fmt, marker::PhantomData};

pub use default::*;

/// The ValidationContext is a collection of slices needed to validate a block
pub trait ValidationContext:
    PotsSlice
    + UtxoSlice
    + PoolsSlice
    + AccountsSlice
    + DRepsSlice
    + CommitteeSlice
    + WitnessSlice
    + ProposalsSlice
{
    type FinalState;
}

/// The PreparationContext is a collection of interfaces needed to prepare a block
pub trait PreparationContext<'a>:
    PrepareUtxoSlice<'a> + PreparePoolsSlice<'a> + PrepareAccountsSlice<'a> + PrepareDRepsSlice<'a>
{
}

// Errors
// -------------------------------------------------------------------------------------------------

#[derive(thiserror::Error, Debug)]
pub enum RegisterError<ROLE, K> {
    #[error("already registered entity: {0:?}")] // TODO: Use Display
    AlreadyRegistered(PhantomData<ROLE>, K),
}

impl<ROLE, K: fmt::Debug> From<diff_bind::RegisterError<K>> for RegisterError<ROLE, K> {
    fn from(
        diff_bind::RegisterError::AlreadyRegistered(source): diff_bind::RegisterError<K>,
    ) -> Self {
        Self::AlreadyRegistered(PhantomData {}, source)
    }
}

#[derive(thiserror::Error, Debug)]
pub enum UnregisterError<ROLE, K> {
    #[error("unknown entity: {0:?}")] // TODO: Use Display
    Unknown(PhantomData<ROLE>, K),
}

impl<ROLE, K: fmt::Debug> From<diff_bind::BindError<K>> for UnregisterError<ROLE, K> {
    fn from(diff_bind::BindError::AlreadyUnregistered(source): diff_bind::BindError<K>) -> Self {
        Self::Unknown(PhantomData {}, source)
    }
}

#[derive(thiserror::Error, Debug)]
pub enum DelegateError<S, T> {
    #[error("unknown source entity: {0:?}")] // TODO: Use Display
    UnknownSource(S),

    #[error("unknown target entity: {0:?}")] // TODO: Use Display
    UnknownTarget(T),
}

impl<S: fmt::Debug, T: fmt::Debug> From<diff_bind::BindError<S>> for DelegateError<S, T> {
    fn from(diff_bind::BindError::AlreadyUnregistered(source): diff_bind::BindError<S>) -> Self {
        Self::UnknownSource(source)
    }
}

#[derive(thiserror::Error, Debug)]
pub enum UpdateError<S> {
    #[error("unknown source entity: {0:?}")] // TODO: Use Display
    UnknownSource(S),
}

impl<S: fmt::Debug> From<diff_bind::BindError<S>> for UpdateError<S> {
    fn from(diff_bind::BindError::AlreadyUnregistered(source): diff_bind::BindError<S>) -> Self {
        Self::UnknownSource(source)
    }
}

// Pots
// -------------------------------------------------------------------------------------------------

/// An interface for interacting with the protocol pots.
pub trait PotsSlice {
    fn add_fees(&mut self, fees: Lovelace);
}

// UTxO
// -------------------------------------------------------------------------------------------------

// An interface for interacting with a subset of the UTxO state.
pub trait UtxoSlice {
    fn lookup(&self, input: &TransactionInput) -> Option<&TransactionOutput>;
    fn consume(&mut self, input: TransactionInput);
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

    // FIXME: Should yield an error when pool doesn't exists.
    fn retire(&mut self, pool: PoolId, epoch: Epoch);
}

/// An interface to help constructing the concrete PoolsSlice ahead of time.
pub trait PreparePoolsSlice<'a> {
    fn require_pool(&'_ mut self, pool: &'a PoolId);
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

    fn register(
        &mut self,
        credential: StakeCredential,
        state: AccountState,
    ) -> Result<(), RegisterError<AccountState, StakeCredential>>;

    fn delegate_pool(
        &mut self,
        credential: StakeCredential,
        pool: PoolId,
    ) -> Result<(), DelegateError<StakeCredential, PoolId>>;

    fn delegate_vote(
        &mut self,
        credential: StakeCredential,
        drep: DRep,
        pointer: CertificatePointer,
    ) -> Result<(), DelegateError<StakeCredential, DRep>>;

    // FIXME: Should yield an error when account doesn't exists.
    fn unregister(&mut self, credential: StakeCredential);

    fn withdraw_from(&mut self, credential: StakeCredential);
}

/// An interface to help constructing the concrete AccountsSlice ahead of time.
pub trait PrepareAccountsSlice<'a> {
    fn require_account(&'_ mut self, credential: &'a StakeCredential);
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
    fn lookup(&self, credential: &StakeCredential) -> Option<&DRepState>;

    fn register(
        &mut self,
        drep: StakeCredential,
        state: DRepState,
    ) -> Result<(), RegisterError<DRepState, StakeCredential>>;

    fn update(
        &mut self,
        drep: StakeCredential,
        anchor: Option<Anchor>,
    ) -> Result<(), UpdateError<StakeCredential>>;

    fn unregister(&mut self, drep: StakeCredential, refund: Lovelace, pointer: CertificatePointer);

    fn vote(&mut self, drep: StakeCredential);
}

/// An interface to help constructing the concrete DRepsSlice ahead of time.
pub trait PrepareDRepsSlice<'a> {
    fn require_drep(&'_ mut self, credential: &'a StakeCredential);
}

// Constitutional Committee
// -------------------------------------------------------------------------------------------------

#[derive(Debug)]
pub struct CCMember {}

/// An interface for interacting with a subset of the Constitutional Committee members state.
pub trait CommitteeSlice {
    fn delegate_cold_key(
        &mut self,
        cc_member: StakeCredential,
        delegate: StakeCredential,
    ) -> Result<(), DelegateError<StakeCredential, StakeCredential>>;

    fn resign(
        &mut self,
        cc_member: StakeCredential,
        anchor: Option<Anchor>,
    ) -> Result<(), UnregisterError<CCMember, StakeCredential>>;
}

// Governance Proposals
// -------------------------------------------------------------------------------------------------

pub trait ProposalsSlice {
    fn acknowledge(&mut self, id: ProposalId, pointer: ProposalPointer, proposal: Proposal);
}

// Witnesses
// -------------------------------------------------------------------------------------------------

pub trait WitnessSlice {
    /// Indicate that a witness is required to be present (and valid) for the corresponding
    /// set of credentials.
    fn require_witness(&mut self, credential: StakeCredential);

    /// Indicate that a bootstrap witness is required to be present (and valid) for the corresponding
    /// root.
    fn require_bootstrap_witness(&mut self, root: Hash<28>);

    /// Obtain the full list of required signers collected while traversing the transaction.
    fn required_signers(&mut self) -> BTreeSet<Hash<28>>;

    /// Obtain the full list of require scripts collected while traversing the transaction.
    fn required_scripts(&self) -> &BTreeSet<Hash<28>>;

    /// Obtain the full list of required bootstrap witnesses collected while traversing the
    /// transaction.
    fn required_bootstrap_signers(&mut self) -> BTreeSet<Hash<28>>;
}
