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

use std::{
    borrow::BorrowMut,
    collections::{BTreeMap, BTreeSet},
    fmt, io, iter,
    path::{Path, PathBuf},
};

use amaru_kernel::StakeEntry;
use amaru_kernel::{
    CertificatePointer,
    Constitution,
    ConstitutionalCommitteeStatus,
    Epoch,
    EraHistory,
    Lovelace,
    MemoizedTransactionOutput,
    Point,
    PoolId,
    Pots,
    ProposalId,
    ProposalsRoots,
    ProtocolParameters,
    RatificationStatus,
    StakeCredential,
    TransactionInput,
    cbor,
    // NOTE: We have to import cbor as minicbor here because we derive 'Encode' and 'Decode' traits
    // instances for some types, and the macro rule handling that seems to be explicitly looking
    // for 'minicbor' in scope, and not an alias of any sort...
    cbor as minicbor,
};
use columns::*;
use thiserror::Error;

use crate::epoch_transition::GovernanceActivity;

pub mod columns;

mod epoch_transition;
pub use epoch_transition::*;

#[derive(Debug, Error)]
#[error(transparent)]
pub enum OpenErrorKind {
    #[error("IO error with file '{file}'")]
    IO {
        file: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("Ledger store at '{file}' is locked")]
    Locked {
        file: PathBuf,
        #[source]
        source: anyhow::Error,
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
    Undecodable(#[from] cbor::decode::Error),

    #[error("error sending work unit through output port")]
    Send,

    #[error(
        "{}",
        if .0.is_locked() {
            "Failed to connect to the ledger store because it is locked. Another Amaru \
            process may still be using it, or a stale LOCK file may remain after an \
            unclean shutdown. Stop any process using the ledger database before retrying; \
            only remove the LOCK file after confirming no process is using it."
        } else {
            "Failed to create ledger. Did you bootstrap your node?"
        }
    )]
    Open(#[source] OpenErrorKind),

    #[error("error retrieving {0}: {1}")]
    Missing(String, #[source] MissingKind),
}

impl StoreError {
    pub fn missing<T: std::fmt::Debug + 'static>(name: &str) -> Self {
        Self::Missing(name.to_string(), MissingKind { type_name: std::any::type_name::<T>().to_string() })
    }
}

impl OpenErrorKind {
    pub fn io_with_file<P: AsRef<Path>>(file: P, source: io::Error) -> Self {
        Self::IO { file: file.as_ref().to_path_buf(), source }
    }

    pub fn locked<P: AsRef<Path>>(file: P, source: anyhow::Error) -> Self {
        Self::Locked { file: file.as_ref().to_path_buf(), source }
    }

    pub fn is_locked(&self) -> bool {
        matches!(self, Self::Locked { .. })
    }
}

// Types
// ----------------------------------------------------------------------------

/// A simple alias for alleviating the store interface annotations.
pub type Result<A> = std::result::Result<A, StoreError>;

#[cfg(any(test, feature = "test-utils"))]
fn default_read_store_error(method: &str) -> StoreError {
    StoreError::Internal(anyhow::anyhow!(method.to_string()).into())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, cbor::Encode, cbor::Decode)]
pub enum EpochTransitionProgress {
    #[n(0)]
    EpochEnded,
    #[n(1)]
    SnapshotTaken,
}

impl fmt::Display for EpochTransitionProgress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Self::EpochEnded => "Epoch Ended",
                Self::SnapshotTaken => "Snapshot Taken",
            }
        )
    }
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
    #[cfg(not(any(test, feature = "test-utils")))]
    fn next_snapshot(&self, epoch: Epoch) -> Result<()>;

    #[cfg(any(test, feature = "test-utils"))]
    fn next_snapshot(&self, epoch: Epoch) -> Result<()> {
        unimplemented!("Store.next_snapshot({epoch})");
    }

    /// Create a new transaction context. This is used to perform updates on the store.
    ///
    /// Prefer [`Store::with_transaction`] if you can. It ensures the transaction is
    /// always either committed or rolled back, removing the risk of leaking an open
    /// transaction on an early `?` return.
    #[cfg(not(any(test, feature = "test-utils")))]
    fn create_transaction(&self) -> Self::Transaction<'_>;

    #[cfg(any(test, feature = "test-utils"))]
    fn create_transaction(&self) -> Self::Transaction<'_> {
        unimplemented!("Store.create_transaction()");
    }

    /// Run `f` inside a transaction:
    ///
    /// - On `Ok`, the transaction is committed.
    /// - On `Err`, the transaction is dropped and auto-rolled-back by its `Drop` impl.
    ///
    /// This makes it impossible to leak an open transaction through an early `?` return
    /// between `create_transaction()` and `commit()`.
    fn with_transaction<R, E>(
        &self,
        f: impl FnOnce(&Self::Transaction<'_>) -> std::result::Result<R, E>,
    ) -> std::result::Result<R, E>
    where
        E: From<StoreError>,
    {
        let tx = self.create_transaction();
        match f(&tx) {
            Ok(result) => {
                tx.commit()?;
                Ok(result)
            }
            Err(err) => Err(err),
        }
    }

    /// Save one batch of account rows while importing a bootstrap snapshot.
    ///
    /// Snapshot import calls this before the snapshot's protocol parameters have been decoded,
    /// so stores must persist the already-normalized rows directly.
    fn save_bootstrap_accounts(&self, _accounts: impl Iterator<Item = (accounts::Key, accounts::Row)>) -> Result<()> {
        Err(StoreError::Internal("bootstrap account batches are not supported by this store".into()))
    }
}

// ReadStore
// ----------------------------------------------------------------------------

pub trait ReadStore {
    #[cfg(not(any(test, feature = "test-utils")))]
    /// Access the tip of the stable store, corresponding to the latest point that was saved.
    fn tip(&self) -> Result<Point>;

    /// A version of 'tip' with a default implementation to allow writing test mocks more easily.
    #[cfg(any(test, feature = "test-utils"))]
    fn tip(&self) -> Result<Point> {
        unimplemented!("ReadStore.tip()");
    }

    /// Get the current epoch transition progress in the store.
    #[cfg(not(any(test, feature = "test-utils")))]
    fn epoch_transition_progress(&self) -> Result<Option<EpochTransitionProgress>>;

    /// A version of 'epoch_transition_progress' with a default implementation to allow writing test mocks more easily.
    #[cfg(any(test, feature = "test-utils"))]
    fn epoch_transition_progress(&self) -> Result<Option<EpochTransitionProgress>> {
        unimplemented!("ReadStore.epoch_transition_progress()");
    }

    /// Get the current protocol parameters
    #[cfg(not(any(test, feature = "test-utils")))]
    fn protocol_parameters(&self) -> Result<ProtocolParameters>;

    /// A version of 'protocol_parameters' with a default implementation to allow writing test mocks more easily.
    #[cfg(any(test, feature = "test-utils"))]
    fn protocol_parameters(&self) -> Result<ProtocolParameters> {
        unimplemented!("ReadStore.protocol_parameters()");
    }

    /// Get details about a specific Pool
    #[cfg(not(any(test, feature = "test-utils")))]
    fn pool(&self, pool: &PoolId) -> Result<Option<pools::Row>>;

    #[cfg(any(test, feature = "test-utils"))]
    fn pool(&self, pool: &PoolId) -> Result<Option<pools::Row>> {
        unimplemented!("ReadStore.pool({pool:?})");
    }

    /// Get details about a specific Account
    #[cfg(not(any(test, feature = "test-utils")))]
    fn account(&self, credential: &StakeCredential) -> Result<Option<accounts::Row>>;

    #[cfg(any(test, feature = "test-utils"))]
    fn account(&self, credential: &StakeCredential) -> Result<Option<accounts::Row>> {
        unimplemented!("ReadStore.account({credential:?})");
    }

    #[cfg(not(any(test, feature = "test-utils")))]
    fn drep(&self, credential: &StakeCredential) -> Result<Option<dreps::Row>>;

    #[cfg(any(test, feature = "test-utils"))]
    fn drep(&self, credential: &StakeCredential) -> Result<Option<dreps::Row>> {
        unimplemented!("ReadStore.drep({credential:?})");
    }

    /// Get details about a specific constitutional committee member
    #[cfg(not(any(test, feature = "test-utils")))]
    fn cc_member(&self, credential: &StakeCredential) -> Result<Option<cc_members::Row>>;

    #[cfg(any(test, feature = "test-utils"))]
    fn cc_member(&self, credential: &StakeCredential) -> Result<Option<cc_members::Row>> {
        unimplemented!("ReadStore.cc_member({credential:?})");
    }

    /// Get details about a specific governance proposal
    #[cfg(not(any(test, feature = "test-utils")))]
    fn proposal(&self, id: &ProposalId) -> Result<Option<proposals::Row>>;

    #[cfg(any(test, feature = "test-utils"))]
    fn proposal(&self, id: &ProposalId) -> Result<Option<proposals::Row>> {
        unimplemented!("ReadStore.proposal({id:?})");
    }

    /// Get details about a specific UTxO
    #[cfg(not(any(test, feature = "test-utils")))]
    fn utxo(&self, input: &TransactionInput) -> Result<Option<MemoizedTransactionOutput>>;

    #[cfg(any(test, feature = "test-utils"))]
    fn utxo(&self, input: &TransactionInput) -> Result<Option<MemoizedTransactionOutput>> {
        unimplemented!("ReadStore.utxo({input:?})");
    }

    /// Get current values of the treasury and reserves accounts.
    #[cfg(not(any(test, feature = "test-utils")))]
    fn pots(&self) -> Result<Pots>;

    #[cfg(any(test, feature = "test-utils"))]
    fn pots(&self) -> Result<Pots> {
        unimplemented!("ReadStore.pots()");
    }

    /// Retrieve the state of the constitutional committee.
    #[cfg(not(any(test, feature = "test-utils")))]
    fn constitutional_committee(&self) -> Result<ConstitutionalCommitteeStatus>;

    #[cfg(any(test, feature = "test-utils"))]
    fn constitutional_committee(&self) -> Result<ConstitutionalCommitteeStatus> {
        unimplemented!("ReadStore.constitutional_committee()");
    }

    /// Retrieve the current protocol's constitution
    #[cfg(not(any(test, feature = "test-utils")))]
    fn constitution(&self) -> Result<Constitution>;

    #[cfg(any(test, feature = "test-utils"))]
    fn constitution(&self) -> Result<Constitution> {
        unimplemented!("ReadStore.constitution()");
    }

    /// Get the latest governance roots; which corresponds to the id of the latest governance
    /// actions enacted for specific categories.
    #[cfg(not(any(test, feature = "test-utils")))]
    fn proposals_roots(&self) -> Result<ProposalsRoots>;

    #[cfg(any(test, feature = "test-utils"))]
    fn proposals_roots(&self) -> Result<ProposalsRoots> {
        unimplemented!("ReadStore.proposals_roots()");
    }

    /// Restore the current governance activity for this epoch.
    #[cfg(not(any(test, feature = "test-utils")))]
    fn governance_activity(&self) -> Result<GovernanceActivity>;

    #[cfg(any(test, feature = "test-utils"))]
    fn governance_activity(&self) -> Result<GovernanceActivity> {
        unimplemented!("ReadStore.governance_activity()");
    }

    /// Get details about all utxos
    #[cfg(not(any(test, feature = "test-utils")))]
    fn iter_utxos(&self) -> Result<impl Iterator<Item = (utxo::Key, utxo::Value)>>;

    #[cfg(any(test, feature = "test-utils"))]
    fn iter_utxos(&self) -> Result<impl Iterator<Item = (utxo::Key, utxo::Value)>> {
        Err::<std::iter::Empty<(utxo::Key, utxo::Value)>, _>(default_read_store_error("ReadStore.iter_utxos()"))
    }

    /// A non-allocating and specialized version of iter_utxos bespoke to the stake distribution
    /// calculations.
    #[cfg(not(any(test, feature = "test-utils")))]
    fn iter_stake_distribution(&self) -> Result<impl Iterator<Item = StakeEntry>>;

    #[cfg(any(test, feature = "test-utils"))]
    fn iter_stake_distribution(&self) -> Result<impl Iterator<Item = StakeEntry>> {
        Err::<std::iter::Empty<StakeEntry>, _>(default_read_store_error("ReadStore.iter_stake_distribution()"))
    }

    /// Get details about all slot leaders
    #[cfg(not(any(test, feature = "test-utils")))]
    fn iter_block_issuers(&self) -> Result<impl Iterator<Item = (slots::Key, slots::Value)>>;

    #[cfg(any(test, feature = "test-utils"))]
    fn iter_block_issuers(&self) -> Result<impl Iterator<Item = (slots::Key, slots::Value)>> {
        Err::<std::iter::Empty<(slots::Key, slots::Value)>, _>(default_read_store_error(
            "ReadStore.iter_block_issuers()",
        ))
    }

    /// Get details about all Pools
    #[cfg(not(any(test, feature = "test-utils")))]
    fn iter_pools(&self) -> Result<impl Iterator<Item = (pools::Key, pools::Row)>>;

    #[cfg(any(test, feature = "test-utils"))]
    fn iter_pools(&self) -> Result<impl Iterator<Item = (pools::Key, pools::Row)>> {
        Err::<std::iter::Empty<(pools::Key, pools::Row)>, _>(default_read_store_error("ReadStore.iter_pools()"))
    }

    /// Get details about all accounts
    #[cfg(not(any(test, feature = "test-utils")))]
    fn iter_accounts(&self) -> Result<impl Iterator<Item = (accounts::Key, accounts::Row)>>;

    #[cfg(any(test, feature = "test-utils"))]
    fn iter_accounts(&self) -> Result<impl Iterator<Item = (accounts::Key, accounts::Row)>> {
        Err::<std::iter::Empty<(accounts::Key, accounts::Row)>, _>(default_read_store_error(
            "ReadStore.iter_accounts()",
        ))
    }

    /// Get details about all recently unregistered accounts
    #[cfg(not(any(test, feature = "test-utils")))]
    fn iter_recently_unregistered_accounts(&self) -> Result<impl Iterator<Item = recently_unregistered_accounts::Key>>;

    #[cfg(any(test, feature = "test-utils"))]
    fn iter_recently_unregistered_accounts(&self) -> Result<impl Iterator<Item = recently_unregistered_accounts::Key>> {
        Err::<std::iter::Empty<recently_unregistered_accounts::Key>, _>(default_read_store_error(
            "ReadStore.iter_recently_unregistered_accounts()",
        ))
    }

    /// Get details about all dreps
    #[cfg(not(any(test, feature = "test-utils")))]
    fn iter_dreps(&self) -> Result<impl Iterator<Item = (dreps::Key, dreps::Row)>>;

    #[cfg(any(test, feature = "test-utils"))]
    fn iter_dreps(&self) -> Result<impl Iterator<Item = (dreps::Key, dreps::Row)>> {
        Err::<std::iter::Empty<(dreps::Key, dreps::Row)>, _>(default_read_store_error("ReadStore.iter_dreps()"))
    }

    /// Get details about all proposals
    #[cfg(not(any(test, feature = "test-utils")))]
    fn iter_proposals(&self) -> Result<impl Iterator<Item = (proposals::Key, proposals::Row)>>;

    #[cfg(any(test, feature = "test-utils"))]
    fn iter_proposals(&self) -> Result<impl Iterator<Item = (proposals::Key, proposals::Row)>> {
        Err::<std::iter::Empty<(proposals::Key, proposals::Row)>, _>(default_read_store_error(
            "ReadStore.iter_proposals()",
        ))
    }

    /// Get proposals that were *just* pruned at last epoch boundary. The list changes every epoch.
    #[cfg(not(any(test, feature = "test-utils")))]
    fn iter_recently_pruned_proposals(
        &self,
    ) -> Result<impl Iterator<Item = (recently_pruned_proposals::Key, recently_pruned_proposals::Value)>>;

    #[cfg(any(test, feature = "test-utils"))]
    fn iter_recently_pruned_proposals(
        &self,
    ) -> Result<impl Iterator<Item = (recently_pruned_proposals::Key, recently_pruned_proposals::Value)>> {
        Err::<std::iter::Empty<(recently_pruned_proposals::Key, recently_pruned_proposals::Value)>, _>(
            default_read_store_error("ReadStore.iter_recently_pruned_proposals()"),
        )
    }

    /// Iterate over constitutional committee members.
    #[cfg(not(any(test, feature = "test-utils")))]
    fn iter_cc_members(&self) -> Result<impl Iterator<Item = (cc_members::Key, cc_members::Row)>>;

    #[cfg(any(test, feature = "test-utils"))]
    fn iter_cc_members(&self) -> Result<impl Iterator<Item = (cc_members::Key, cc_members::Row)>> {
        Err::<std::iter::Empty<(cc_members::Key, cc_members::Row)>, _>(default_read_store_error(
            "ReadStore.iter_cc_members()",
        ))
    }

    /// Iterate over votes.
    #[cfg(not(any(test, feature = "test-utils")))]
    fn iter_votes(&self) -> Result<impl Iterator<Item = (votes::Key, votes::Row)>>;

    #[cfg(any(test, feature = "test-utils"))]
    fn iter_votes(&self) -> Result<impl Iterator<Item = (votes::Key, votes::Row)>> {
        Err::<std::iter::Empty<(votes::Key, votes::Row)>, _>(default_read_store_error("ReadStore.iter_votes()"))
    }
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
            .unwrap_or_else(|| panic!("called 'least_recent_snapshot' on empty database?!"))
    }

    /// The most recent snapshot. Note that we never starts from genesis; so there's always a
    /// snapshot available.
    #[expect(clippy::panic)]
    fn most_recent_snapshot(&self) -> Epoch {
        self.snapshots()
            .unwrap_or_default()
            .last()
            .copied()
            .unwrap_or_else(|| panic!("called 'most_recent_snapshot' on empty database?!"))
    }

    /// Access a `Snapshot` for a specific `Epoch`
    fn for_epoch(&self, epoch: Epoch) -> Result<impl Snapshot + Send>;
}

// TransactionalContext
// ----------------------------------------------------------------------------

/// A trait that provides a handle to perform atomic updates on the store.
pub trait TransactionalContext<'a>: ReadStore {
    /// Commit the transaction. This will persist all changes to the store.
    #[cfg(not(any(test, feature = "test-utils")))]
    fn commit(self) -> Result<()>;

    #[cfg(any(test, feature = "test-utils"))]
    fn commit(self) -> Result<()>
    where
        Self: Sized,
    {
        unimplemented!("TransactionalContext.commit()");
    }

    /// Rollback the transaction. This will not persist any changes to the store.
    #[cfg(not(any(test, feature = "test-utils")))]
    fn rollback(self) -> Result<()>;

    #[cfg(any(test, feature = "test-utils"))]
    fn rollback(self) -> Result<()>
    where
        Self: Sized,
    {
        unimplemented!("TransactionalContext.rollback()");
    }

    /// Idempotently reset the epoch transition progress.
    #[cfg(not(any(test, feature = "test-utils")))]
    fn reset_epoch_transition_progress(&self) -> Result<()>;

    #[cfg(any(test, feature = "test-utils"))]
    fn reset_epoch_transition_progress(&self) -> Result<()> {
        unimplemented!("TransactionalContext.reset_epoch_transition_progress()");
    }

    /// Try to update the epoch transition progress so that we can recover from interruption within an
    /// epoch transition, if this ever happens.
    ///
    /// - return `True` and updates the store if the progress before the call matched the `from` argument.
    /// - returns `False` and does not update the store otherwise.
    #[cfg(not(any(test, feature = "test-utils")))]
    fn try_epoch_transition(
        &self,
        from: Option<EpochTransitionProgress>,
        to: Option<EpochTransitionProgress>,
    ) -> Result<bool>;

    #[cfg(any(test, feature = "test-utils"))]
    fn try_epoch_transition(
        &self,
        from: Option<EpochTransitionProgress>,
        to: Option<EpochTransitionProgress>,
    ) -> Result<bool> {
        unimplemented!("TransactionalContext.try_epoch_transition({from:?}, {to:?})");
    }

    /// Add or remove entries to/from the store. The exact semantic of 'add' and 'remove' depends
    /// on the column type. All updates are atomatic and attached to the given `Point`.
    ///
    /// `governance_activity` is `None` for saves that carry no governance state (e.g. a raw UTxO
    /// import): such saves leave the persisted dormant-epoch counter untouched.
    #[expect(clippy::too_many_arguments)]
    #[cfg(not(any(test, feature = "test-utils")))]
    fn save(
        &self,
        era_history: &EraHistory,
        protocol_parameters: &ProtocolParameters,
        governance_activity: Option<GovernanceActivity>,
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

    #[expect(clippy::too_many_arguments)]
    #[cfg(any(test, feature = "test-utils"))]
    fn save(
        &self,
        _era_history: &EraHistory,
        _protocol_parameters: &ProtocolParameters,
        _governance_activity: Option<GovernanceActivity>,
        point: &Point,
        _issuer: Option<&pools::Key>,
        _add: Columns<
            impl Iterator<Item = (utxo::Key, utxo::Value)>,
            impl Iterator<Item = pools::Value>,
            impl Iterator<Item = (accounts::Key, accounts::Value)>,
            impl Iterator<Item = (dreps::Key, dreps::Value)>,
            impl Iterator<Item = (cc_members::Key, cc_members::Value)>,
            impl Iterator<Item = (proposals::Key, proposals::Value)>,
            impl Iterator<Item = (votes::Key, votes::Value)>,
        >,
        _remove: Columns<
            impl Iterator<Item = utxo::Key>,
            impl Iterator<Item = (pools::Key, Epoch)>,
            impl Iterator<Item = accounts::Key>,
            impl Iterator<Item = (dreps::Key, CertificatePointer)>,
            impl Iterator<Item = cc_members::Key>,
            impl Iterator<Item = ()>,
            impl Iterator<Item = ()>,
        >,
        _withdrawals: impl Iterator<Item = accounts::Key>,
    ) -> Result<()> {
        unimplemented!("TransactionalContext.save({point})");
    }

    /// Refund a deposit into an account. If the account no longer exists, returns the unrefunded
    /// deposit.
    #[cfg(not(any(test, feature = "test-utils")))]
    fn refund(&self, credential: &accounts::Key, deposit: Lovelace) -> Result<Lovelace>;

    #[cfg(any(test, feature = "test-utils"))]
    fn refund(&self, credential: &accounts::Key, deposit: Lovelace) -> Result<Lovelace> {
        unimplemented!("TransactionalContext.refund({credential:?}, {deposit})");
    }

    /// Persist ProtocolParameters for the ongoing epoch.
    #[cfg(not(any(test, feature = "test-utils")))]
    fn set_protocol_parameters(&self, protocol_parameters: &ProtocolParameters) -> Result<()>;

    #[cfg(any(test, feature = "test-utils"))]
    fn set_protocol_parameters(&self, protocol_parameters: &ProtocolParameters) -> Result<()> {
        unimplemented!("TransactionalContext.set_protocol_parameters({protocol_parameters:?})");
    }

    /// Persist the constitutional committee state for the ongoing epoch.
    #[cfg(not(any(test, feature = "test-utils")))]
    fn update_constitutional_committee(
        &self,
        status: &ConstitutionalCommitteeStatus,
        added: &BTreeMap<StakeCredential, Epoch>,
        removed: &BTreeSet<StakeCredential>,
    ) -> Result<()>;

    #[cfg(any(test, feature = "test-utils"))]
    fn update_constitutional_committee(
        &self,
        status: &ConstitutionalCommitteeStatus,
        added: &BTreeMap<StakeCredential, Epoch>,
        removed: &BTreeSet<StakeCredential>,
    ) -> Result<()> {
        unimplemented!("TransactionalContext.update_constitutional_committee({status:?}, {added:?}, {removed:?})");
    }

    /// Persist the latest proposal roots for the ongoing epoch.
    #[cfg(not(any(test, feature = "test-utils")))]
    fn set_proposals_roots(&self, roots: &ProposalsRoots) -> Result<()>;

    #[cfg(any(test, feature = "test-utils"))]
    fn set_proposals_roots(&self, roots: &ProposalsRoots) -> Result<()> {
        unimplemented!("TransactionalContext.set_proposals_roots({roots:?})");
    }

    /// Persist the latest enacted constitution
    #[cfg(not(any(test, feature = "test-utils")))]
    fn set_constitution(&self, constitution: &Constitution) -> Result<()>;

    #[cfg(any(test, feature = "test-utils"))]
    fn set_constitution(&self, constitution: &Constitution) -> Result<()> {
        unimplemented!("TransactionalContext.set_constitution({constitution:?})");
    }

    /// Track the current governance activity.
    #[cfg(not(any(test, feature = "test-utils")))]
    fn set_governance_activity(&self, dormant_epochs: GovernanceActivity) -> Result<()>;

    #[cfg(any(test, feature = "test-utils"))]
    fn set_governance_activity(&self, dormant_epochs: GovernanceActivity) -> Result<()> {
        unimplemented!("TransactionalContext.set_governance_activity({dormant_epochs:?})");
    }

    /// Record the recently (i.e. last epoch boundary) pruned proposals
    #[cfg(not(any(test, feature = "test-utils")))]
    fn set_recently_pruned_proposals<'iter>(
        &self,
        proposals: impl IntoIterator<Item = (&'iter ProposalId, RatificationStatus)>,
    ) -> Result<()>;

    #[cfg(any(test, feature = "test-utils"))]
    fn set_recently_pruned_proposals<'iter>(
        &self,
        proposals: impl IntoIterator<Item = (&'iter ProposalId, RatificationStatus)>,
    ) -> Result<()> {
        unimplemented!(
            "TransactionalContext.set_recently_pruned_proposals({:?})",
            proposals.into_iter().collect::<Vec<_>>()
        );
    }

    /// Remove a list of proposals from the database. This is done when enacting proposals that
    /// cause other proposals to become obsolete.
    #[cfg(not(any(test, feature = "test-utils")))]
    fn remove_proposals<T>(&self, proposals: &BTreeMap<ProposalId, T>) -> Result<()>;

    #[cfg(any(test, feature = "test-utils"))]
    fn remove_proposals<T>(&self, proposals: &BTreeMap<ProposalId, T>) -> Result<()> {
        unimplemented!("TransactionalContext.remove_proposals({:?})", proposals.keys().collect::<Vec<_>>());
    }

    /// Prune all recently unregistered accounts from the database that are no longer required to
    /// keep around.
    #[cfg(not(any(test, feature = "test-utils")))]
    fn prune_recently_unregistered_accounts(&self, epoch: Epoch) -> Result<()>;

    #[cfg(any(test, feature = "test-utils"))]
    fn prune_recently_unregistered_accounts(&self, epoch: Epoch) -> Result<()> {
        unimplemented!("TransactionalContext.prune_recently_unregistered_accounts({epoch})");
    }

    /// Get current values of the treasury and reserves accounts, and possibly modify them.
    #[cfg(not(any(test, feature = "test-utils")))]
    fn with_pots(&self, with: impl FnMut(Box<dyn BorrowMut<pots::Row> + '_>)) -> Result<()>;

    #[cfg(any(test, feature = "test-utils"))]
    fn with_pots(&self, _with: impl FnMut(Box<dyn BorrowMut<pots::Row> + '_>)) -> Result<()> {
        unimplemented!("TransactionalContext.with_pots()");
    }

    /// Provide an access to iterate over pools, in a way that enforces:
    ///
    /// 1. That mutations will be persisted on-disk
    ///
    /// 2. That all operations are consistent and atomic (the iteration occurs on a snapshot, and
    ///    the mutation apply to the iterated items)
    #[cfg(not(any(test, feature = "test-utils")))]
    fn with_pools(&self, with: impl FnMut(pools::Iter<'_, '_>)) -> Result<()>;

    #[cfg(any(test, feature = "test-utils"))]
    fn with_pools(&self, _with: impl FnMut(pools::Iter<'_, '_>)) -> Result<()> {
        unimplemented!("TransactionalContext.with_pools()");
    }

    /// Provide an access to iterate over accounts, similar to 'with_pools'.
    #[cfg(not(any(test, feature = "test-utils")))]
    fn with_accounts(&self, with: impl FnMut(accounts::Iter<'_, '_>)) -> Result<()>;

    #[cfg(any(test, feature = "test-utils"))]
    fn with_accounts(&self, _with: impl FnMut(accounts::Iter<'_, '_>)) -> Result<()> {
        unimplemented!("TransactionalContext.with_accounts()");
    }

    /// Provide an iterator over slot leaders, similar to 'with_pools'. Note that slot leaders are
    /// stored as a bounded FIFO, so it only make sense to use this function at the end of an epoch
    /// (or at the beginning, before any block is applied, depending on your perspective).
    #[cfg(not(any(test, feature = "test-utils")))]
    fn with_block_issuers(&self, with: impl FnMut(slots::Iter<'_, '_>)) -> Result<()>;

    #[cfg(any(test, feature = "test-utils"))]
    fn with_block_issuers(&self, _with: impl FnMut(slots::Iter<'_, '_>)) -> Result<()> {
        unimplemented!("TransactionalContext.with_block_issuers()");
    }

    /// Provide an access to iterate over utxo, similar to 'with_pools'.
    #[cfg(not(any(test, feature = "test-utils")))]
    fn with_utxo(&self, with: impl FnMut(utxo::Iter<'_, '_>)) -> Result<()>;

    #[cfg(any(test, feature = "test-utils"))]
    fn with_utxo(&self, _with: impl FnMut(utxo::Iter<'_, '_>)) -> Result<()> {
        unimplemented!("TransactionalContext.with_utxo()");
    }

    /// Provide an access to iterate over dreps, similar to 'with_pools'.
    #[cfg(not(any(test, feature = "test-utils")))]
    fn with_dreps(&self, with: impl FnMut(dreps::Iter<'_, '_>)) -> Result<()>;

    #[cfg(any(test, feature = "test-utils"))]
    fn with_dreps(&self, _with: impl FnMut(dreps::Iter<'_, '_>)) -> Result<()> {
        unimplemented!("TransactionalContext.with_dreps()");
    }

    /// Provide an access to iterate over cc members, similar to 'with_pools'.
    #[cfg(not(any(test, feature = "test-utils")))]
    fn with_cc_members(&self, with: impl FnMut(cc_members::Iter<'_, '_>)) -> Result<()>;

    #[cfg(any(test, feature = "test-utils"))]
    fn with_cc_members(&self, _with: impl FnMut(cc_members::Iter<'_, '_>)) -> Result<()> {
        unimplemented!("TransactionalContext.with_cc_members()");
    }
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

#[cfg(test)]
mod tests {
    use anyhow::anyhow;

    use super::*;

    #[test]
    fn better_context_on_open_locked() {
        let error = StoreError::Open(OpenErrorKind::locked(PathBuf::from("db/live"), anyhow!("lock held")));
        let message = format!("{error:#}");
        assert!(message.contains("Failed to connect to the ledger store because it is locked"));
        assert!(!message.contains("Did you bootstrap your node?"));
    }

    #[test]
    fn suggest_bootstrap_on_open_error() {
        let error = StoreError::Open(OpenErrorKind::io_with_file(PathBuf::from("db/live"), io::Error::other("foo")));
        let message = format!("{error:#}");
        assert!(!message.contains("Failed to connect to the ledger store because it is locked"));
        assert!(message.contains("Did you bootstrap your node?"));
    }
}
