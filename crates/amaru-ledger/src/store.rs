// Copyright 2024 PRAGMA
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

pub mod columns;

use crate::{
    governance::ratification::{ProposalsRoots, ProposalsRootsRc},
    summary::Pots,
};
use amaru_kernel::{
    CertificatePointer,
    EraHistory,
    Lovelace,
    Point,
    PoolId,
    StakeCredential,
    TransactionInput,
    // NOTE: We have to import cbor as minicbor here because we derive 'Encode' and 'Decode' traits
    // instances for some types, and the macro rule handling that seems to be explicitly looking
    // for 'minicbor' in scope, and not an alias of any sort...
    cbor as minicbor,
    protocol_parameters::ProtocolParameters,
};
use amaru_kernel::{
    ComparableProposalId, Constitution, ConstitutionalCommitteeStatus, Epoch,
    MemoizedTransactionOutput,
};
use columns::*;
use std::{
    borrow::BorrowMut,
    collections::{BTreeMap, BTreeSet},
    io, iter,
    ops::Deref,
    path::Path,
};
use thiserror::Error;

#[derive(Debug, Error)]
#[error(transparent)]
pub enum OpenErrorKind {
    #[error("IO error with file '{file}': {source}")]
    IO {
        file: String,
        #[source]
        source: io::Error,
    },
    #[error("no ledger stable snapshot found; at least two are expected")]
    NoStableSnapshot,
}

#[derive(Debug, Error)]
#[error("no database {type_name}. Did you forget to 'import' a snapshot first?")]
pub struct MissingKind {
    type_name: String,
}

#[derive(Error, Debug)]
pub enum StoreError {
    #[error(transparent)]
    Internal(#[from] Box<dyn std::error::Error + Send + Sync>),

    #[error("unable to decode database's value: {0}")]
    Undecodable(#[from] minicbor::decode::Error),

    #[error("error sending work unit through output port")]
    Send,

    #[error("error opening the store: {0}")]
    Open(#[source] OpenErrorKind),

    #[error("error retrieving {0}: {1}")]
    Missing(String, #[source] MissingKind),
}

impl StoreError {
    pub fn missing<T: std::fmt::Debug + 'static>(name: &str) -> Self {
        Self::Missing(
            name.to_string(),
            MissingKind {
                type_name: std::any::type_name::<T>().to_string(),
            },
        )
    }
}

impl OpenErrorKind {
    pub fn io_with_file<P: AsRef<Path>>(file: P, error: io::Error) -> Self {
        Self::IO {
            file: file.as_ref().display().to_string(),
            source: error,
        }
    }
}

// Types
// ----------------------------------------------------------------------------

/// A simple alias for alleviating the store interface annotations.
pub type Result<A> = std::result::Result<A, StoreError>;

#[derive(Debug, PartialEq, Eq, minicbor::Encode, minicbor::Decode, Clone)]
pub enum EpochTransitionProgress {
    #[n(0)]
    EpochEnded,
    #[n(1)]
    SnapshotTaken,
    #[n(2)]
    EpochStarted,
}

#[derive(Debug, PartialEq, Eq, minicbor::Encode, minicbor::Decode, Clone)]
pub struct GovernanceActivity {
    #[n(0)]
    pub consecutive_dormant_epochs: u32,
}

// Snapshot
// ----------------------------------------------------------------------------

pub trait Snapshot: ReadStore {
    fn epoch(&self) -> Epoch;
}

// Store
// ----------------------------------------------------------------------------

pub trait Store: ReadStore {
    type Transaction<'a>: TransactionalContext<'a> + ReadStore
    where
        Self: 'a;

    /// Construct and save on-disk a snapshot of the store. The epoch number is used when
    /// there's no existing snapshot and, to ensure that snapshots are taken in order.
    ///
    /// Idempotent
    ///
    /// /!\ IMPORTANT /!\
    /// It is the **caller's** responsibility to ensure that the snapshot is done at the right
    /// moment. The store has no notion of when is an epoch boundary, and thus deferred that
    /// decision entirely to the caller owning the store.
    fn next_snapshot(&self, epoch: Epoch) -> Result<()>;

    /// Create a new transaction context. This is used to perform updates on the store.
    fn create_transaction(&self) -> Self::Transaction<'_>;
}

// ReadStore
// ----------------------------------------------------------------------------

pub trait ReadStore {
    /// Access the tip of the stable store, corresponding to the latest point that was saved.
    fn tip(&self) -> Result<Point>;

    /// Get the current protocol parameters
    fn protocol_parameters(&self) -> Result<ProtocolParameters>;

    /// Get details about a specific Pool
    fn pool(&self, pool: &PoolId) -> Result<Option<pools::Row>>;

    /// Get details about a specific Account
    fn account(&self, credential: &StakeCredential) -> Result<Option<accounts::Row>>;

    /// Get details about a specific UTxO
    fn utxo(&self, input: &TransactionInput) -> Result<Option<MemoizedTransactionOutput>>;

    /// Get current values of the treasury and reserves accounts.
    fn pots(&self) -> Result<Pots>;

    /// Retrieve the state of the constitutional committee.
    fn constitutional_committee(&self) -> Result<ConstitutionalCommitteeStatus>;

    /// Retrieve the current protocol's constitution
    fn constitution(&self) -> Result<Constitution>;

    /// Get the latest governance roots; which corresponds to the id of the latest governance
    /// actions enacted for specific categories.
    fn proposals_roots(&self) -> Result<ProposalsRoots>;

    /// Restore the current governance activity for this epoch.
    fn governance_activity(&self) -> Result<GovernanceActivity>;

    /// Get details about all utxos
    fn iter_utxos(&self) -> Result<impl Iterator<Item = (utxo::Key, utxo::Value)>>;

    /// Get details about all slot leaders
    fn iter_block_issuers(&self) -> Result<impl Iterator<Item = (slots::Key, slots::Value)>>;

    /// Get details about all Pools
    fn iter_pools(&self) -> Result<impl Iterator<Item = (pools::Key, pools::Row)>>;

    /// Get details about all accounts
    fn iter_accounts(&self) -> Result<impl Iterator<Item = (accounts::Key, accounts::Row)>>;

    /// Get details about all dreps
    fn iter_dreps(&self) -> Result<impl Iterator<Item = (dreps::Key, dreps::Row)>>;

    /// Get details about all proposals
    fn iter_proposals(&self) -> Result<impl Iterator<Item = (proposals::Key, proposals::Row)>>;

    /// Iterate over constitutional committee members.
    fn iter_cc_members(&self) -> Result<impl Iterator<Item = (cc_members::Key, cc_members::Row)>>;

    /// Iterate over votes.
    fn iter_votes(&self) -> Result<impl Iterator<Item = (votes::Key, votes::Row)>>;
}

// HistoricalStores
// ----------------------------------------------------------------------------

pub trait HistoricalStores {
    /// Get a list of all snapshots available. The list is ordered from the oldest to the newest.
    fn snapshots(&self) -> Result<Vec<Epoch>>;

    /// Prune snapshot older than the given epoch (excluded). This shall keep snapshots *at* the
    /// provided epoch.
    fn prune(&self, minimum_epoch: Epoch) -> Result<()>;

    /// The least recent snapshot. Note that we never starts from genesis; so there's always a
    /// snapshot available.
    #[expect(clippy::panic)]
    fn least_recent_snapshot(&self) -> Epoch {
        self.snapshots()
            .unwrap_or_default()
            .first()
            .copied()
            .unwrap_or_else(|| panic!("called 'epoch' on empty database?!"))
    }

    /// The most recent snapshot. Note that we never starts from genesis; so there's always a
    /// snapshot available.
    #[expect(clippy::panic)]
    fn most_recent_snapshot(&self) -> Epoch {
        self.snapshots()
            .unwrap_or_default()
            .last()
            .copied()
            .unwrap_or_else(|| panic!("called 'epoch' on empty database?!"))
    }

    /// Access a `Snapshot` for a specific `Epoch`
    fn for_epoch(&self, epoch: Epoch) -> Result<impl Snapshot>;
}

// TransactionalContext
// ----------------------------------------------------------------------------

/// A trait that provides a handle to perform atomic updates on the store.
pub trait TransactionalContext<'a>: ReadStore {
    /// Commit the transaction. This will persist all changes to the store.
    fn commit(self) -> Result<()>;

    /// Rollback the transaction. This will not persist any changes to the store.
    fn rollback(self) -> Result<()>;

    /// Try to update the epoch transition progress so that we can recover from interruption within an
    /// epoch transition, if this ever happens.
    ///
    /// - return `True` and updates the store if the progress before the call matched the `from` argument.
    /// - returns `False` and does not update the store otherwise.
    fn try_epoch_transition(
        &self,
        from: Option<EpochTransitionProgress>,
        to: Option<EpochTransitionProgress>,
    ) -> Result<bool>;

    /// Add or remove entries to/from the store. The exact semantic of 'add' and 'remove' depends
    /// on the column type. All updates are atomatic and attached to the given `Point`.
    #[expect(clippy::too_many_arguments)]
    fn save(
        &self,
        era_history: &EraHistory,
        protocol_parameters: &ProtocolParameters,
        governance_activity: &mut GovernanceActivity,
        point: &Point,
        issuer: Option<&pools::Key>,
        add: Columns<
            impl Iterator<Item = (utxo::Key, utxo::Value)>,
            impl Iterator<Item = pools::Value>,
            impl Iterator<Item = (accounts::Key, accounts::Value)>,
            impl Iterator<Item = (dreps::Key, dreps::Value)>,
            impl Iterator<Item = (cc_members::Key, cc_members::Value)>,
            impl Iterator<Item = (proposals::Key, proposals::Value)>,
            impl Iterator<Item = (votes::Key, votes::Value)>,
        >,
        remove: Columns<
            impl Iterator<Item = utxo::Key>,
            impl Iterator<Item = (pools::Key, Epoch)>,
            impl Iterator<Item = accounts::Key>,
            impl Iterator<Item = (dreps::Key, CertificatePointer)>,
            impl Iterator<Item = cc_members::Key>,
            impl Iterator<Item = ()>,
            impl Iterator<Item = ()>,
        >,
        withdrawals: impl Iterator<Item = accounts::Key>,
    ) -> Result<()>;

    /// Refund a deposit into an account. If the account no longer exists, returns the unrefunded
    /// deposit.
    fn refund(&self, credential: &accounts::Key, deposit: Lovelace) -> Result<Lovelace>;

    /// Persist ProtocolParameters for the ongoing epoch.
    fn set_protocol_parameters(&self, protocol_parameters: &ProtocolParameters) -> Result<()>;

    /// Persist the constitutional committee state for the ongoing epoch.
    fn update_constitutional_committee(
        &self,
        status: &ConstitutionalCommitteeStatus,
        added: BTreeMap<StakeCredential, Epoch>,
        removed: BTreeSet<StakeCredential>,
    ) -> Result<()>;

    /// Persist the latest proposal roots for the ongoing epoch.
    fn set_proposals_roots(&self, roots: &ProposalsRootsRc) -> Result<()>;

    /// Persist the latest enacted constitution
    fn set_constitution(&self, constitution: &Constitution) -> Result<()>;

    /// Track the current governance activity.
    fn set_governance_activity(&self, dormant_epochs: &GovernanceActivity) -> Result<()>;

    /// Remove a list of proposals from the database. This is done when enacting proposals that
    /// cause other proposals to become obsolete.
    fn remove_proposals<'iter, Id>(&self, proposals: impl IntoIterator<Item = Id>) -> Result<()>
    where
        Id: Deref<Target = ComparableProposalId> + 'iter;

    /// Get current values of the treasury and reserves accounts, and possibly modify them.
    fn with_pots(&self, with: impl FnMut(Box<dyn BorrowMut<pots::Row> + '_>)) -> Result<()>;

    /// Provide an access to iterate over pools, in a way that enforces:
    ///
    /// 1. That mutations will be persisted on-disk
    ///
    /// 2. That all operations are consistent and atomic (the iteration occurs on a snapshot, and
    ///    the mutation apply to the iterated items)
    fn with_pools(&self, with: impl FnMut(pools::Iter<'_, '_>)) -> Result<()>;

    /// Provide an access to iterate over accounts, similar to 'with_pools'.
    fn with_accounts(&self, with: impl FnMut(accounts::Iter<'_, '_>)) -> Result<()>;

    /// Provide an iterator over slot leaders, similar to 'with_pools'. Note that slot leaders are
    /// stored as a bounded FIFO, so it only make sense to use this function at the end of an epoch
    /// (or at the beginning, before any block is applied, depending on your perspective).
    fn with_block_issuers(&self, with: impl FnMut(slots::Iter<'_, '_>)) -> Result<()>;

    /// Provide an access to iterate over utxo, similar to 'with_pools'.
    fn with_utxo(&self, with: impl FnMut(utxo::Iter<'_, '_>)) -> Result<()>;

    /// Provide an access to iterate over dreps, similar to 'with_pools'.
    fn with_dreps(&self, with: impl FnMut(dreps::Iter<'_, '_>)) -> Result<()>;

    /// Provide an access to iterate over dreps, similar to 'with_pools'.
    fn with_proposals(&self, with: impl FnMut(proposals::Iter<'_, '_>)) -> Result<()>;

    /// Provide an access to iterate over cc members, similar to 'with_pools'.
    fn with_cc_members(&self, with: impl FnMut(cc_members::Iter<'_, '_>)) -> Result<()>;
}

// Columns
// ----------------------------------------------------------------------------

/// A summary of all database columns, in a single struct. This can be derived to provide updates
/// operations on multiple columns in a single db-transaction.
pub struct Columns<U, P, A, D, C, PP, V> {
    pub utxo: U,
    pub pools: P,
    pub accounts: A,
    pub dreps: D,
    pub cc_members: C,
    pub proposals: PP,
    pub votes: V,
}

impl<U, P, A, D, C, PP, V> Default
    for Columns<
        iter::Empty<U>,
        iter::Empty<P>,
        iter::Empty<A>,
        iter::Empty<D>,
        iter::Empty<C>,
        iter::Empty<PP>,
        iter::Empty<V>,
    >
{
    fn default() -> Self {
        Self {
            utxo: iter::empty(),
            pools: iter::empty(),
            accounts: iter::empty(),
            dreps: iter::empty(),
            cc_members: iter::empty(),
            proposals: iter::empty(),
            votes: iter::empty(),
        }
    }
}

impl<U, P, A, D, C, PP, V> Columns<U, P, A, D, C, PP, V> {
    pub fn empty() -> Columns<
        std::iter::Empty<U>,
        std::iter::Empty<P>,
        std::iter::Empty<A>,
        std::iter::Empty<D>,
        std::iter::Empty<C>,
        std::iter::Empty<PP>,
        std::iter::Empty<V>,
    > {
        Columns {
            utxo: std::iter::empty(),
            pools: std::iter::empty(),
            accounts: std::iter::empty(),
            dreps: std::iter::empty(),
            cc_members: std::iter::empty(),
            proposals: std::iter::empty(),
            votes: std::iter::empty(),
        }
    }
}
