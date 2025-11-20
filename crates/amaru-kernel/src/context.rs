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

// FIXME: This is probably not the right place for this to live.
//
// We need the `ValidationContext` to be accessible in both `amaru-plutus` and `amaru-ledger`
// so the quick fix was to move it here.
//
// It probably makes the most sense to move it to it's own crate

use crate::{
    AddrKeyhash, Anchor, CertificatePointer, DRep, DRepRegistration, DatumHash, Hash, Lovelace,
    MemoizedDatum, MemoizedPlutusData, MemoizedScript, MemoizedTransactionOutput, PoolId,
    PoolParams, Proposal, ProposalId, ProposalPointer, RequiredScript, ScriptHash, StakeCredential,
    TransactionInput, Vote, Voter, arc_mapped::ArcMapped, diff_bind,
};
use amaru_slot_arithmetic::Epoch;
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    marker::PhantomData,
    sync::Arc,
};

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
    fn lookup(&self, input: &TransactionInput) -> Option<&MemoizedTransactionOutput>;

    fn get(&self, input: &TransactionInput) -> Option<Arc<MemoizedTransactionOutput>>;

    fn consume(
        &mut self,
        input: TransactionInput,
    ) -> Option<(&TransactionInput, Arc<MemoizedTransactionOutput>)>;

    fn produce(
        &mut self,
        input: TransactionInput,
        output: MemoizedTransactionOutput,
    ) -> Arc<MemoizedTransactionOutput>;
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

    fn register(&mut self, params: PoolParams, pointer: CertificatePointer);

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
    pub pool: Option<(PoolId, CertificatePointer)>,
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
        pointer: CertificatePointer,
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

/// An interface for interacting with a subset of the DReps state.
pub trait DRepsSlice {
    fn lookup(&self, credential: &StakeCredential) -> Option<&DRepRegistration>;

    fn register(
        &mut self,
        drep: StakeCredential,
        registration: DRepRegistration,
        anchor: Option<Anchor>,
    ) -> Result<(), RegisterError<DRepRegistration, StakeCredential>>;

    fn update(
        &mut self,
        drep: StakeCredential,
        anchor: Option<Anchor>,
    ) -> Result<(), UpdateError<StakeCredential>>;

    fn unregister(&mut self, drep: StakeCredential, refund: Lovelace, pointer: CertificatePointer);
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

    fn vote(&mut self, proposal: ProposalId, voter: Voter, vote: Vote, anchor: Option<Anchor>);
}

// Witnesses
// -------------------------------------------------------------------------------------------------

pub trait WitnessSlice {
    /// Indicate a datum that may appear in the witness set that isn't required for spending an input
    fn allow_supplemental_datum(&mut self, datum_hash: Hash<32>);

    /// Acknowledge presence of a script in the transaction by its location, to use for later validations.
    fn acknowledge_script(&mut self, script_hash: ScriptHash, location: TransactionInput);

    /// Acknowledge presence of an inline datum in the transaction by its location, to use for later validations.
    fn acknowledge_datum(&mut self, datum_hash: DatumHash, location: TransactionInput);

    /// Indicate that a script wintess is required to be present (and valid) for the corresponding script data
    fn require_script_witness(&mut self, script: RequiredScript);

    /// Indicate that a vkey witness is required to be present (and valid) for the corresponding
    /// key hash.
    fn require_vkey_witness(&mut self, vkey_hash: AddrKeyhash);

    /// Indicate that a bootstrap witness is required to be present (and valid) for the corresponding
    /// root.
    fn require_bootstrap_witness(&mut self, root: Hash<28>);

    /// Obtain the full list of allowed supplemental datums while traversing the transaction
    fn allowed_supplemental_datums(&mut self) -> BTreeSet<Hash<32>>;

    /// Obtain the full list of required signers collected while traversing the transaction.
    fn required_signers(&mut self) -> BTreeSet<Hash<28>>;

    /// Obtain the full list of require scripts collected while traversing the transaction.
    fn required_scripts(&mut self) -> BTreeSet<RequiredScript>;

    /// Obtain the full list of required bootstrap witnesses collected while traversing the
    /// transaction.
    fn required_bootstrap_roots(&mut self) -> BTreeSet<Hash<28>>;

    /// Obtain the full list of known scripts collected while traversing the transaction.
    fn known_scripts(&mut self) -> BTreeMap<ScriptHash, &MemoizedScript>;

    /// Obtain the full list of known datums collected while traversing the transaction.
    fn known_datums(
        &mut self,
    ) -> BTreeMap<DatumHash, ArcMapped<MemoizedTransactionOutput, MemoizedPlutusData>>;
}

/// Implement 'known_script' using the provided script locations and a context that is at least a
/// UtxoSlice.
///
/// Note that re-constructing the known scripts is relatively fast as the lookup are in logarithmic
/// times, and no allocation (other than the BTreeMap) is happening whatsoever.
pub fn blanket_known_scripts<C>(
    context: &'_ mut C,
    known_scripts: impl Iterator<Item = (ScriptHash, TransactionInput)>,
) -> BTreeMap<ScriptHash, &'_ MemoizedScript>
where
    C: UtxoSlice,
{
    let mut scripts = BTreeMap::new();

    for (script_hash, location) in known_scripts {
        let lookup = |input| {
            UtxoSlice::lookup(context, input)
                .and_then(|output| output.script.as_ref())
                .unwrap_or_else(|| unreachable!("no script at expected location: {location:?}"))
        };

        scripts.insert(script_hash, lookup(&location));
    }

    scripts
}

/// Implement 'known_datums' using the provided datum locations and a context that is at least a
/// UtxoSlice.
///
/// Note that re-constructing the known datums is relatively fast as the lookup are in logarithmic
/// times, and no allocation (other than the BTreeMap) is happening whatsoever.
pub fn blanket_known_datums<C>(
    context: &'_ mut C,
    known_datums: impl Iterator<Item = (DatumHash, TransactionInput)>,
) -> BTreeMap<DatumHash, ArcMapped<MemoizedTransactionOutput, MemoizedPlutusData>>
where
    C: UtxoSlice,
{
    let mut datums = BTreeMap::new();

    for (datum_hash, location) in known_datums {
        let lookup = |input| {
            let output = UtxoSlice::get(context, input)
                .unwrap_or_else(|| unreachable!("no expected location: {location:?}"));

            ArcMapped::new(output.clone(), &EXTRACT_INLINE_DATUM)
        };

        datums.insert(datum_hash, lookup(&location));
    }

    datums
}

pub static EXTRACT_INLINE_DATUM: fn(&MemoizedTransactionOutput) -> &MemoizedPlutusData =
    |output| match &output.datum {
        MemoizedDatum::None | MemoizedDatum::Hash(..) => {
            unreachable!("no datum at expected location")
        }
        MemoizedDatum::Inline(data) => data,
    };
