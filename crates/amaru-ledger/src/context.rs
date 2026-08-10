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
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
    fmt,
    marker::PhantomData,
};

use amaru_kernel::{
    Anchor, CertificatePointer, DRep, DRepRegistration, Epoch, Hash, Lovelace, MemoizedDatum, MemoizedPlutusData,
    MemoizedScript, MemoizedTransactionOutput, Mint, PoolId, PoolParams, Proposal, ProposalId, ProposalKind,
    ProposalPointer, ProposalsRoots, RequiredScript, StakeCredential, TransactionInput, Value, Vote, Voter,
    cardano::value::Balance,
    size::{DATUM, KEY, SCRIPT},
};
use thiserror::Error;

use crate::{state::volatile, store::StoreError};

mod default;
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
    + BalanceSlice
{
    type FinalState;
}

/// The PreparationContext is a collection of interfaces needed to prepare a block
pub trait PreparationContext<'a>:
    PrepareUtxoSlice<'a>
    + PreparePoolsSlice<'a>
    + PrepareAccountsSlice<'a>
    + PrepareDRepsSlice<'a>
    + PrepareCommitteeSlice<'a>
    + PrepareProposalsSlice<'a>
{
}

/// Local enum deciding what we should do for unresolved inputs happening when validating transactions.
/// If we are validating transactions from a block we can defer the check because the inputs might
/// be provided by transactions in the same block.
///
/// However, there are cases (such as mempool validation) where a missing input may not be acceptable.
#[derive(Debug, Clone, Copy)]
pub enum UnresolvedInputPolicy {
    Defer,
    Reject,
}

// Errors (preparation)
// -------------------------------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum ContextHydratationError {
    #[error("failed to hydrate inputs")]
    ResolveInputs(#[source] StoreError),

    #[error("unknown (but required) transaction input or reference input: {0}")]
    UnknownInput(TransactionInput),

    #[error("failed to hydrate pools")]
    ResolvePools(#[source] StoreError),

    #[error("failed to hydrate accounts")]
    ResolveAccounts(#[source] StoreError),

    #[error("failed to hydrate dreps")]
    ResolveDReps(#[source] StoreError),

    #[error("failed to hydrate committee members")]
    ResolveCommittee(#[source] StoreError),

    #[error("failed to hydrate proposals")]
    ResolveProposals(#[source] StoreError),

    #[error("failed to hydrate pots")]
    ResolvePots(#[source] StoreError),
}

// Errors (validation)
// -------------------------------------------------------------------------------------------------

#[derive(thiserror::Error, Debug)]
pub enum RegisterError<ROLE, K> {
    #[error("already registered entity: {0:?}")] // TODO: Use Display
    AlreadyRegistered(PhantomData<ROLE>, K),
}

impl<ROLE, K: fmt::Debug> From<volatile::RegisterError<K>> for RegisterError<ROLE, K> {
    fn from(volatile::RegisterError::AlreadyRegistered(source): volatile::RegisterError<K>) -> Self {
        Self::AlreadyRegistered(PhantomData {}, source)
    }
}

#[derive(thiserror::Error, Debug)]
pub enum UnregisterError<ROLE, K> {
    #[error("unknown entity: {0:?}")] // TODO: Use Display
    Unknown(PhantomData<ROLE>, K),
}

impl<ROLE, K: fmt::Debug> From<volatile::BindError<K>> for UnregisterError<ROLE, K> {
    fn from(volatile::BindError::AlreadyUnregistered(source): volatile::BindError<K>) -> Self {
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

impl<S: fmt::Debug, T: fmt::Debug> From<volatile::BindError<S>> for DelegateError<S, T> {
    fn from(volatile::BindError::AlreadyUnregistered(source): volatile::BindError<S>) -> Self {
        Self::UnknownSource(source)
    }
}

#[derive(thiserror::Error, Debug)]
pub enum UpdateError<S> {
    #[error("unknown source entity: {0:?}")] // TODO: Use Display
    UnknownSource(S),
}

impl<S: fmt::Debug> From<volatile::BindError<S>> for UpdateError<S> {
    fn from(volatile::BindError::AlreadyUnregistered(source): volatile::BindError<S>) -> Self {
        Self::UnknownSource(source)
    }
}

// Pots
// -------------------------------------------------------------------------------------------------

/// An interface for interacting with the protocol pots.
pub trait PotsSlice {
    /// The treasury value as of the start of the current epoch; constant within an epoch.
    fn treasury(&self) -> Lovelace;
    fn add_fees(&mut self, fees: Lovelace);
    fn add_donation(&mut self, donation: Lovelace);
}

// UTxO
// -------------------------------------------------------------------------------------------------

/// An interface for interacting with a subset of the UTxO state.
pub trait UtxoSlice {
    fn lookup(&self, input: &TransactionInput) -> Option<&MemoizedTransactionOutput>;
    fn consume(&mut self, input: TransactionInput);
    fn produce(&mut self, input: TransactionInput, output: MemoizedTransactionOutput);
}

/// An interface to help constructing the concrete UtxoSlice ahead of time.
pub trait PrepareUtxoSlice<'a> {
    fn require_input(&'_ mut self, input: &'a TransactionInput);
}

// Pools
// ------------------------------------------------------------------------------------------------

/// An interface for interacting with a subset of the Pools state.
pub trait PoolsSlice {
    fn exists(&self, pool: PoolId) -> bool;

    fn register(&mut self, params: PoolParams, pointer: CertificatePointer, deposit: Lovelace);

    fn retire(&mut self, pool: PoolId, epoch: Epoch) -> Result<(), UnregisterError<PoolId, PoolId>>;
}

/// An interface to help constructing the concrete PoolsSlice ahead of time.
pub trait PreparePoolsSlice<'a> {
    fn require_pool(&'_ mut self, pool: &'a PoolId);
}

// Accounts
// ------------------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct AccountState {
    pub deposit: Lovelace,
    pub pool: Option<(PoolId, CertificatePointer)>,
    pub drep: Option<(DRep, CertificatePointer)>,
    /// Withdrawable reward balance as of the start of the block; a withdrawal must claim exactly
    /// this. `0` for a freshly registered account.
    pub rewards: Lovelace,
}

/// An interface for interacting with a subset of the Accounts state.
pub trait AccountsSlice {
    /// The account state at this point in the block; the resolved block-start state with the
    /// in-block changes folded in.
    fn lookup(&self, credential: &StakeCredential) -> Option<AccountState>;

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
    fn require_account(&mut self, credential: Cow<'a, StakeCredential>);
}

// DRep
// -------------------------------------------------------------------------------------------------

/// An interface for interacting with a subset of the DReps state.
pub trait DRepsSlice {
    /// The DRep registration at this point in the block: the block-start record, or a fresh in-block
    /// registration that supersedes it. Unlike accounts this never merges, so it returns a reference.
    fn lookup(&self, credential: &StakeCredential) -> Option<&DRepRegistration>;

    fn register(
        &mut self,
        drep: StakeCredential,
        registration: DRepRegistration,
        anchor: Option<Box<Anchor>>,
    ) -> Result<(), RegisterError<DRepRegistration, StakeCredential>>;

    fn update(
        &mut self,
        drep: StakeCredential,
        anchor: Option<Box<Anchor>>,
    ) -> Result<(), UpdateError<StakeCredential>>;

    fn unregister(&mut self, drep: StakeCredential, refund: Lovelace, pointer: CertificatePointer);
}

/// An interface to help constructing the concrete DRepsSlice ahead of time.
pub trait PrepareDRepsSlice<'a> {
    fn require_drep(&'_ mut self, credential: &'a StakeCredential);

    /// Require the DRep targeted by a vote delegation. The credential is constructed from the `DRep`
    /// at resolution (and `Abstain`/`NoConfidence` drop out), so the borrowed `DRep` is collected.
    fn require_drep_delegation(&'_ mut self, drep: &'a DRep);
}

// Constitutional Committee
// -------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct CCMember {
    /// The authorized hot credential, if the member has declared one.
    pub hot_credential: Option<StakeCredential>,
    /// The term expiry; `None` once the member is inactive (no-confidence).
    pub valid_until: Option<Epoch>,
}

/// An interface for interacting with a subset of the Constitutional Committee members state.
pub trait CommitteeSlice {
    /// The committee member at this point in the block: the block-start record with the in-block
    /// hot-key change folded in, or `None` once it has resigned.
    fn lookup(&self, cc_member: &StakeCredential) -> Option<CCMember>;

    fn delegate_cold_key(
        &mut self,
        cc_member: StakeCredential,
        delegate: StakeCredential,
    ) -> Result<(), DelegateError<StakeCredential, StakeCredential>>;

    fn resign(
        &mut self,
        cc_member: StakeCredential,
        anchor: Option<Box<Anchor>>,
    ) -> Result<(), UnregisterError<CCMember, StakeCredential>>;
}

/// An interface to help constructing the concrete CommitteeSlice ahead of time.
pub trait PrepareCommitteeSlice<'a> {
    fn require_committee_member(&'_ mut self, cc_member: &'a StakeCredential);
}

// Governance Proposals
// -------------------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ProposalState {
    pub proposed_in: ProposalPointer,
    /// Last epoch the proposal can be voted on.
    pub valid_until: Epoch,
    pub proposal: Proposal,
}

pub trait ProposalsSlice {
    /// Whether an active proposal `id` exists AND shares `kind`'s governance purpose (same
    /// [`ProposalKind`] discriminant). Folds block-start proposals together with ones acknowledged
    /// earlier in the block.
    fn exists(&self, id: &ProposalId, kind: &ProposalKind) -> bool;

    /// The current governance roots, i.e. the latest enacted action per category.
    fn roots(&self) -> &ProposalsRoots;

    fn acknowledge(&mut self, id: ProposalId, pointer: ProposalPointer, proposal: Proposal);

    fn vote(&mut self, proposal: ProposalId, voter: Voter, vote: Vote, anchor: Option<Anchor>);
}

/// An interface to help constructing the concrete ProposalsSlice ahead of time.
pub trait PrepareProposalsSlice<'a> {
    fn require_proposal(&'_ mut self, id: &'a ProposalId);
}

// Value Preservation
// -------------------------------------------------------------------------------------------------

/// An interface for accumulating the running total of value produced and consumed by a
/// transaction. A valid transaction should have a balance of zero.
///
/// A negative balance means there is more value in the "right-hand side" (outputs + fees + treasury donation + negative mints)
/// A positive balance means there is more value in the "left-hand side" (inputs + refunds + positive mints)
pub trait BalanceSlice {
    /// Add a non-negative [`Value`] to the balance accumulator.
    fn consume_value(&mut self, value: &Value);

    /// Subtract a non-negative [`Value`] to the balance accumulator.
    fn produce_value(&mut self, value: &Value);

    /// Add a lovelace amount to the balance accumulator.
    fn consume_lovelace(&mut self, amount: Lovelace);

    /// Subtract a lovelace amount to the balance accumulator.
    fn produce_lovelace(&mut self, amount: Lovelace);

    /// Add a signed `Mint` to the balance accumulator. Positive entries
    /// represent newly minted tokens, negative entries represent burns.
    fn add_mint(&mut self, mint: &Mint);

    /// The current total balance.
    fn balance(&mut self) -> Balance;
}

// Witnesses
// -------------------------------------------------------------------------------------------------

pub trait WitnessSlice {
    /// Indicate a datum that may appear in the witness set that isn't required for spending an input
    fn allow_supplemental_datum(&mut self, datum_hash: Hash<DATUM>);

    /// Acknowledge presence of a script in the transaction by its location, to use for later validations.
    fn acknowledge_script(&mut self, script_hash: Hash<SCRIPT>, location: TransactionInput);

    /// Acknowledge presence of an inline datum in the transaction by its location, to use for later validations.
    fn acknowledge_datum(&mut self, datum_hash: Hash<DATUM>, location: TransactionInput);

    /// Indicate that a script wintess is required to be present (and valid) for the corresponding script data
    fn require_script_witness(&mut self, script: RequiredScript);

    /// Indicate that a verification key witness is required to be present (and valid) for the corresponding
    /// key hash.
    fn require_verification_key_witness(&mut self, verification_key_hash: Hash<KEY>);

    /// Indicate that a bootstrap witness is required to be present (and valid) for the corresponding
    /// root.
    fn require_bootstrap_witness(&mut self, root: Hash<28>);

    /// Obtain the full list of allowed supplemental datums while traversing the transaction
    fn allowed_supplemental_datums(&mut self) -> BTreeSet<Hash<DATUM>>;

    /// Obtain the full list of required signers collected while traversing the transaction.
    fn required_signers(&mut self) -> BTreeSet<Hash<KEY>>;

    /// Obtain the full list of require scripts collected while traversing the transaction.
    fn required_scripts(&mut self) -> BTreeSet<RequiredScript>;

    /// Obtain the full list of required bootstrap witnesses collected while traversing the
    /// transaction.
    fn required_bootstrap_roots(&mut self) -> BTreeSet<Hash<28>>;

    /// Obtain the full list of known scripts collected while traversing the transaction.
    fn known_scripts(&mut self) -> BTreeMap<Hash<SCRIPT>, &MemoizedScript>;

    /// Obtain the full list of known datums collected while traversing the transaction.
    fn known_datums(&mut self) -> BTreeMap<Hash<DATUM>, &MemoizedPlutusData>;
}

/// Implement 'known_script' using the provided script locations and a context that is at least a
/// UtxoSlice.
///
/// Note that re-constructing the known scripts is relatively fast as the lookup are in logarithmic
/// times, and no allocation (other than the BTreeMap) is happening whatsoever.
pub fn blanket_known_scripts<C>(
    context: &'_ mut C,
    known_scripts: impl Iterator<Item = (Hash<SCRIPT>, TransactionInput)>,
) -> BTreeMap<Hash<SCRIPT>, &'_ MemoizedScript>
where
    C: UtxoSlice,
{
    let mut scripts = BTreeMap::new();

    for (script_hash, location) in known_scripts {
        let lookup = |input| {
            UtxoSlice::lookup(context, input)
                .and_then(|output| output.script.as_deref())
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
    known_datums: impl Iterator<Item = (Hash<DATUM>, TransactionInput)>,
) -> BTreeMap<Hash<DATUM>, &'_ MemoizedPlutusData>
where
    C: UtxoSlice,
{
    let mut datums = BTreeMap::new();

    for (datum_hash, location) in known_datums {
        let lookup = |input| {
            UtxoSlice::lookup(context, input)
                .and_then(|output| match &output.datum {
                    MemoizedDatum::None => None,
                    MemoizedDatum::Hash(..) => None,
                    MemoizedDatum::Inline(data) => Some(data.as_ref()),
                })
                .unwrap_or_else(|| unreachable!("no datum at expected location: {location:?}"))
        };

        datums.insert(datum_hash, lookup(&location));
    }

    datums
}
