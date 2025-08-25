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

use ::rocksdb::{self, checkpoint, OptimisticTransactionDB, Options, SliceTransform};
use amaru_iter_borrow::{self, borrowable_proxy::BorrowableProxy, IterBorrow};
use amaru_kernel::{
    cbor, protocol_parameters::ProtocolParameters, CertificatePointer, ComparableProposalId,
    Constitution, ConstitutionalCommittee, EraHistory, Lovelace, MemoizedTransactionOutput, Point,
    PoolId, StakeCredential, TransactionInput,
};
use amaru_ledger::{
    governance::ratification::{ProposalsRoots, ProposalsRootsRc},
    store::{
        columns as scolumns, Columns, EpochTransitionProgress, GovernanceActivity,
        HistoricalStores, OpenErrorKind, ReadStore, Snapshot, Store, StoreError,
        TransactionalContext,
    },
    summary::Pots,
};
use amaru_slot_arithmetic::Epoch;
use rocksdb::{
    DBAccess, DBIteratorWithThreadMode, Direction, IteratorMode, ReadOptions, Transaction, DB,
};
use std::{
    fmt, fs,
    ops::Deref,
    path::{Path, PathBuf},
};
use tracing::{info, instrument, trace, warn, Level};

pub mod ledger;
use ledger::columns::*;

pub mod common;
use common::{as_value, PREFIX_LEN};

pub mod consensus;

mod transaction;
use transaction::OngoingTransaction;

// Constants
// ----------------------------------------------------------------------------

const EVENT_TARGET: &str = "amaru::ledger::store";

/// Key where is stored the tip of the database (most recently applied delta)
const KEY_TIP: &str = "@tip";

/// key where is stored the progress of the database
const KEY_PROGRESS: &str = "@progress";

/// Key where is stored the current protocol parameters
const KEY_PROTOCOL_PARAMETERS: &str = "@protocol-parameters";

/// key where is stored the constitutional committee information;
const KEY_CONSTITUTIONAL_COMMITTEE: &str = "@constitutional-committee";

/// key where is stored the constitution
const KEY_CONSTITUTION: &str = "@constitution";

/// key where are stored the proposals roots;
const KEY_PROPOSALS_ROOTS: &str = "@proposals-roots";

/// key where is stored the governance activity (e.g. number of dormant epochs).
const KEY_GOVERNANCE_ACTIVITY: &str = "@governance-activity";

/// Name of the directory containing the live ledger stable database.
const DIR_LIVE_DB: &str = "live";

// RocksDB
// ----------------------------------------------------------------------------

// An opaque handle for a store implementation of top of RocksDB. The database has the
// following structure:
//
// * ===========================*================================================ *
// * key                        * value                                           *
// * ===========================*================================================ *
// * '@tip'                      * Point                                          *
// * '@progress'                 * EpochTransitionProgress                        *
// * '@pots'                     * (Lovelace, Lovelace, Lovelace)                 *
// * '@protocol-version'         * ProtocolVersion                                *
// * '@protocol-parameters'      * ProtocolParameters                             *
// * '@governance-activity'      * GovernanceActivity                             *
// * '@constitutional-committee' * ConstitutionalCommittee                        *
// * '@constitutional'           * Constitution                                   *
// * 'utxo:'TransactionInput     * TransactionOutput                              *
// * 'pool:'PoolId               * (PoolParams, Vec<(Option<PoolParams>, Epoch)>) *
// * 'acct:'StakeCredential      * (Option<PoolId>, Lovelace, Lovelace)           *
// * 'drep:'StakeCredential      * (                                              *
// *                             *   Lovelace,                                    *
// *                             *   Option<Anchor>,                              *
// *                             *   CertificatePointer,                          *
// *                             *   Option<Epoch>,                               *
// *                             *   Option<CertificatePointer>,                  *
// *                             * )                                              *
// * 'comm:'StakeCredential      * (Option<StakeCredential>)                      *
// * 'prop:'ProposalId           * (ProposalPointer, Epoch, Proposal)             *
// * 'vote:'Voter                * Ballot                                         *
// * 'slot':slot                 * PoolId                                         *
// * ===========================*================================================ *
//
// CBOR is used to serialize objects (as keys or values) into their binary equivalent.
pub struct RocksDB {
    /// The working directory where we store the various key/value stores.
    dir: PathBuf,

    /// Whether to allow saving data incrementally (i.e. calling multiple times 'save' with the
    /// same point). This is only sound when importing static data, but not when running live.
    incremental_save: bool,

    /// An instance of RocksDB.
    db: OptimisticTransactionDB,

    ongoing_transaction: OngoingTransaction,
}

impl RocksDB {
    pub fn snapshots(dir: &Path) -> Result<Vec<Epoch>, StoreError> {
        let mut snapshots: Vec<Epoch> = Vec::new();

        for entry in fs::read_dir(dir)
            .map_err(|err| StoreError::Open(OpenErrorKind::IO(err)))?
            .by_ref()
        {
            let entry = entry.map_err(|err| StoreError::Open(OpenErrorKind::IO(err)))?;
            if let Ok(epoch) = entry
                .file_name()
                .to_str()
                .unwrap_or_default()
                .parse::<Epoch>()
            {
                snapshots.push(epoch);
            } else if entry.file_name() != DIR_LIVE_DB {
                warn!(
                    target: EVENT_TARGET,
                    filename = entry.file_name().to_str().unwrap_or_default(),
                    "new.unexpected_file"
                );
            }
        }

        snapshots.sort();

        Ok(snapshots)
    }

    pub fn new(dir: &Path) -> Result<Self, StoreError> {
        assert_sufficient_snapshots(dir)?;
        let mut opts = default_opts_with_prefix();
        opts.create_if_missing(true);
        OptimisticTransactionDB::open(&opts, dir.join("live"))
            .map(|db| Self {
                dir: dir.to_path_buf(),
                incremental_save: false,
                db,
                ongoing_transaction: OngoingTransaction::new(),
            })
            .map_err(|err| StoreError::Internal(err.into()))
    }

    pub fn empty(dir: &Path) -> Result<RocksDB, StoreError> {
        let mut opts = Options::default();
        opts.create_if_missing(true);
        opts.set_prefix_extractor(SliceTransform::create_fixed_prefix(PREFIX_LEN));
        OptimisticTransactionDB::open(&opts, dir.join("live"))
            .map(|db| Self {
                dir: dir.to_path_buf(),
                incremental_save: true,
                db,
                ongoing_transaction: OngoingTransaction::new(),
            })
            .map_err(|err| StoreError::Internal(err.into()))
    }

    fn transaction_ended(&self) {
        self.ongoing_transaction.set(false);
    }
}

// RocksDBReadOnly
// ----------------------------------------------------------------------------

/// A version of the RocksDB implementation that holds a read-only connection. Useful for
/// monitoring tools / API that are merely inspecting the database.
pub struct ReadOnlyRocksDB {
    db: DB,
}

impl ReadOnlyRocksDB {
    pub fn new(dir: &Path) -> Result<Self, StoreError> {
        assert_sufficient_snapshots(dir)?;
        let opts = default_opts_with_prefix();
        rocksdb::DB::open_for_read_only(&opts, dir.join("live"), false)
            .map(|db| ReadOnlyRocksDB { db })
            .map_err(|err| StoreError::Internal(err.into()))
    }
}

// Snapshot
// ----------------------------------------------------------------------------

pub struct RocksDBSnapshot {
    epoch: Epoch,
    db: OptimisticTransactionDB,
}

impl Snapshot for RocksDBSnapshot {
    fn epoch(&'_ self) -> Epoch {
        self.epoch
    }
}

// Store
// ----------------------------------------------------------------------------

impl Store for RocksDB {
    type Transaction<'a> = RocksDBTransactionalContext<'a>;

    #[instrument(level = Level::INFO, target = EVENT_TARGET, name = "snapshot", skip_all, fields(epoch))]
    fn next_snapshot(&'_ self, epoch: Epoch) -> Result<(), StoreError> {
        let path = self.dir.join(epoch.to_string());

        if path.exists() {
            // RocksDB error can't be created externally, so panic instead
            // It might be better to come up with a global error type
            fs::remove_dir_all(&path).map_err(|_| {
                StoreError::Internal("Unable to remove existing snapshot directory".into())
            })?;
        }

        checkpoint::Checkpoint::new(&self.db)
            .and_then(|handle| handle.create_checkpoint(path))
            .map_err(|err| StoreError::Internal(err.into()))?;

        Ok(())
    }

    #[allow(clippy::panic)] // Expected
    fn create_transaction(&self) -> RocksDBTransactionalContext<'_> {
        if self.ongoing_transaction.get() {
            // Thats a bug in the code, we should never have two transactions at the same time
            panic!("RocksDB already has an ongoing transaction");
        }
        let transaction = self.db.transaction();
        self.ongoing_transaction.set(true);
        RocksDBTransactionalContext {
            host: self,
            db: transaction,
        }
    }
}

// HistoricalStores
// ----------------------------------------------------------------------------

pub struct RocksDBHistoricalStores {
    dir: PathBuf,
}

impl RocksDBHistoricalStores {
    pub fn for_epoch_with(base_dir: &Path, epoch: Epoch) -> Result<RocksDBSnapshot, StoreError> {
        let opts = default_opts_with_prefix();

        OptimisticTransactionDB::open(&opts, base_dir.join(PathBuf::from(format!("{epoch}"))))
            .map_err(|err| StoreError::Internal(err.into()))
            .map(|db| RocksDBSnapshot { epoch, db })
    }

    pub fn new(dir: &Path) -> Self {
        RocksDBHistoricalStores {
            dir: dir.to_path_buf(),
        }
    }
}

impl HistoricalStores for RocksDBHistoricalStores {
    fn snapshots(&self) -> Result<Vec<Epoch>, StoreError> {
        let mut snapshots: Vec<Epoch> = Vec::new();

        for entry in fs::read_dir(&self.dir)
            .map_err(|err| StoreError::Open(OpenErrorKind::IO(err)))?
            .by_ref()
        {
            let entry = entry.map_err(|err| StoreError::Open(OpenErrorKind::IO(err)))?;
            if let Ok(epoch) = entry
                .file_name()
                .to_str()
                .unwrap_or_default()
                .parse::<Epoch>()
            {
                snapshots.push(epoch);
            } else if entry.file_name() != DIR_LIVE_DB {
                warn!(
                    target: EVENT_TARGET,
                    filename = entry.file_name().to_str().unwrap_or_default(),
                    "new.unexpected_file"
                );
            }
        }

        snapshots.sort();

        Ok(snapshots)
    }
    fn for_epoch(&self, epoch: Epoch) -> Result<impl Snapshot, StoreError> {
        RocksDBHistoricalStores::for_epoch_with(&self.dir, epoch)
    }
}

// ReadStore(s)
// ----------------------------------------------------------------------------

macro_rules! impl_ReadStore {
    // with lifetimes/generics: e.g., for<'a> Foo<'a>
    (for<$($gen:tt),*> $t:ty) => {
        impl_ReadStore_body!{ impl<$($gen,)*> ReadStore for $t }
    };

    // plain type: e.g., Foo
    ($t:ty) => {
        impl_ReadStore_body!{ impl ReadStore for $t }
    };
}

macro_rules! impl_ReadStore_body {
    ($($header:tt)*) => {
        $($header)* {
            fn tip(&self) -> Result<Point, StoreError> {
                get_or_bail(|key| self.db.get(key), KEY_TIP)
            }

            fn protocol_parameters(
                &self,
            ) -> Result<ProtocolParameters, StoreError> {
                get_or_bail(|key| self.db.get(key), &KEY_PROTOCOL_PARAMETERS)
            }

            fn constitutional_committee(&self) -> Result<ConstitutionalCommittee, StoreError> {
                get_or_bail(|key| self.db.get(key), &KEY_CONSTITUTIONAL_COMMITTEE)
            }

            fn constitution(&self) -> Result<Constitution, StoreError> {
                get_or_bail(|key| self.db.get(key), &KEY_CONSTITUTION)
            }

            fn governance_activity(&self) -> Result<GovernanceActivity, StoreError> {
                Ok(get(|key| self.db.get(key), &KEY_GOVERNANCE_ACTIVITY)?
                    .unwrap_or_else(|| GovernanceActivity { consecutive_dormant_epochs: 0 })
                )
            }

            fn proposals_roots(&self) -> Result<ProposalsRoots, StoreError> {
                get_or_bail(|key| self.db.get(key), &KEY_PROPOSALS_ROOTS)
            }

            fn pool(&self, pool: &PoolId) -> Result<Option<scolumns::pools::Row>, StoreError> {
                pools::get(|key| self.db.get(key), pool)
            }

            fn account(
                &self,
                credential: &StakeCredential,
            ) -> Result<Option<scolumns::accounts::Row>, StoreError> {
                accounts::get(|key| self.db.get(key), credential)
            }

            fn utxo(
                &self,
                input: &TransactionInput,
            ) -> Result<Option<MemoizedTransactionOutput>, StoreError> {
                utxo::get(|key| self.db.get(key), input)
            }

            fn iter_utxos(
                &self,
            ) -> Result<impl Iterator<Item = (scolumns::utxo::Key, scolumns::utxo::Value)>, StoreError>
            {
                iter(
                    |mode, opts| self.db.iterator_opt(mode, opts),
                    utxo::PREFIX,
                    Direction::Forward,
                )
            }

            fn pots(&self) -> Result<Pots, StoreError> {
                pots::get(|key| self.db.get(key)).map(|row| Pots::from(&row))
            }

            fn iter_accounts(
                &self,
            ) -> Result<impl Iterator<Item = (scolumns::accounts::Key, scolumns::accounts::Row)>, StoreError>
            {
                iter(
                    |mode, opts| self.db.iterator_opt(mode, opts),
                    accounts::PREFIX,
                    Direction::Forward,
                )
            }

            fn iter_block_issuers(
                &self,
            ) -> Result<impl Iterator<Item = (scolumns::slots::Key, scolumns::slots::Value)>, StoreError>
            {
                iter(
                    |mode, opts| self.db.iterator_opt(mode, opts),
                    slots::PREFIX,
                    Direction::Forward,
                )
            }

            fn iter_pools(
                &self,
            ) -> Result<impl Iterator<Item = (scolumns::pools::Key, scolumns::pools::Row)>, StoreError>
            {
                iter(
                    |mode, opts| self.db.iterator_opt(mode, opts),
                    pools::PREFIX,
                    Direction::Forward,
                )
            }

            fn iter_dreps(
                &self,
            ) -> Result<impl Iterator<Item = (scolumns::dreps::Key, scolumns::dreps::Row)>, StoreError>
            {
                iter(
                    |mode, opts| self.db.iterator_opt(mode, opts),
                    dreps::PREFIX,
                    Direction::Forward,
                )
            }

            fn iter_proposals(
                &self,
            ) -> Result<
                impl Iterator<Item = (scolumns::proposals::Key, scolumns::proposals::Row)>,
                StoreError,
            > {
                iter(
                    |mode, opts| self.db.iterator_opt(mode, opts),
                    proposals::PREFIX,
                    Direction::Forward,
                )
            }

            fn iter_cc_members(
                &self,
            ) -> Result<
                impl Iterator<Item = (scolumns::cc_members::Key, scolumns::cc_members::Row)>,
                StoreError,
            > {
                iter(
                    |mode, opts| self.db.iterator_opt(mode, opts),
                    cc_members::PREFIX,
                    Direction::Forward,
                )
            }

            fn iter_votes(
                &self,
            ) -> Result<impl Iterator<Item = (scolumns::votes::Key, scolumns::votes::Row)>, StoreError>
            {
                iter(
                    |mode, opts| self.db.iterator_opt(mode, opts),
                    votes::PREFIX,
                    Direction::Forward,
                )
            }
        }
    }
}

impl_ReadStore!(RocksDB);
impl_ReadStore!(RocksDBSnapshot);
impl_ReadStore!(ReadOnlyRocksDB);
impl_ReadStore!(for<'a> RocksDBTransactionalContext<'a>);

// TransactionalContext
// ----------------------------------------------------------------------------

pub struct RocksDBTransactionalContext<'a> {
    host: &'a RocksDB,
    db: Transaction<'a, OptimisticTransactionDB>,
}

impl TransactionalContext<'_> for RocksDBTransactionalContext<'_> {
    fn commit(self) -> Result<(), StoreError> {
        let res = self
            .db
            .commit()
            .map_err(|err| StoreError::Internal(err.into()));
        self.host.transaction_ended();
        res
    }

    fn rollback(mut self) -> Result<(), StoreError> {
        let transaction = std::mem::replace(&mut self.db, self.host.db.transaction());
        let res = transaction
            .rollback()
            .map_err(|err| StoreError::Internal(err.into()));
        self.host.transaction_ended();
        res
    }

    #[instrument(
        level = Level::TRACE,
        skip_all,
    )]
    fn try_epoch_transition(
        &self,
        from: Option<EpochTransitionProgress>,
        to: Option<EpochTransitionProgress>,
    ) -> Result<bool, StoreError> {
        let previous_progress = self
            .db
            .get(KEY_PROGRESS)
            .map_err(|err| StoreError::Internal(err.into()))?
            .map(|bytes| cbor::decode(&bytes))
            .transpose()
            .map_err(StoreError::Undecodable)?;

        if previous_progress != from {
            return Ok(false);
        }

        match to {
            None => self.db.delete(KEY_PROGRESS),
            Some(to) => self.db.put(KEY_PROGRESS, as_value(to)),
        }
        .map_err(|err| StoreError::Internal(err.into()))?;

        Ok(true)
    }

    /// Refund a deposit into an account. If the account no longer exists, returns the unrefunded
    /// deposit.
    fn refund(
        &self,
        credential: &scolumns::accounts::Key,
        deposit: Lovelace,
    ) -> Result<Lovelace, StoreError> {
        accounts::set(&self.db, credential, |balance| balance + deposit)
    }

    fn set_protocol_parameters(
        &self,
        protocol_parameters: &ProtocolParameters,
    ) -> Result<(), StoreError> {
        self.db
            .put(KEY_PROTOCOL_PARAMETERS, as_value(protocol_parameters))
            .map_err(|err| StoreError::Internal(err.into()))?;
        Ok(())
    }

    fn set_constitutional_committee(
        &self,
        constitutional_committee: &ConstitutionalCommittee,
    ) -> Result<(), StoreError> {
        self.db
            .put(
                KEY_CONSTITUTIONAL_COMMITTEE,
                as_value(constitutional_committee),
            )
            .map_err(|err| StoreError::Internal(err.into()))?;
        Ok(())
    }

    fn set_proposals_roots(&self, roots: &ProposalsRootsRc) -> Result<(), StoreError> {
        self.db
            .put(KEY_PROPOSALS_ROOTS, as_value(roots))
            .map_err(|err| StoreError::Internal(err.into()))?;
        Ok(())
    }

    fn set_constitution(&self, constitution: &Constitution) -> Result<(), StoreError> {
        self.db
            .put(KEY_CONSTITUTION, as_value(constitution))
            .map_err(|err| StoreError::Internal(err.into()))?;
        Ok(())
    }

    fn set_governance_activity(
        &self,
        governance_activity: &GovernanceActivity,
    ) -> Result<(), StoreError> {
        self.db
            .put(KEY_GOVERNANCE_ACTIVITY, as_value(governance_activity))
            .map_err(|err| StoreError::Internal(err.into()))?;
        Ok(())
    }

    /// Remove a list of proposals from the database. This is done when enacting proposals that
    /// cause other proposals to become obsolete.
    fn remove_proposals<'iter, Id>(
        &self,
        proposals: impl IntoIterator<Item = Id>,
    ) -> Result<(), StoreError>
    where
        Id: Deref<Target = ComparableProposalId> + 'iter,
    {
        proposals::remove(&self.db, proposals.into_iter())
    }

    fn save(
        &self,
        era_history: &EraHistory,
        protocol_parameters: &ProtocolParameters,
        governance_activity: &mut GovernanceActivity,
        point: &Point,
        issuer: Option<&scolumns::pools::Key>,
        add: Columns<
            impl Iterator<Item = (scolumns::utxo::Key, scolumns::utxo::Value)>,
            impl Iterator<Item = scolumns::pools::Value>,
            impl Iterator<Item = (scolumns::accounts::Key, scolumns::accounts::Value)>,
            impl Iterator<Item = (scolumns::dreps::Key, scolumns::dreps::Value)>,
            impl Iterator<Item = (scolumns::cc_members::Key, scolumns::cc_members::Value)>,
            impl Iterator<Item = (scolumns::proposals::Key, scolumns::proposals::Value)>,
            impl Iterator<Item = (scolumns::votes::Key, scolumns::votes::Value)>,
        >,
        remove: Columns<
            impl Iterator<Item = scolumns::utxo::Key>,
            impl Iterator<Item = (scolumns::pools::Key, Epoch)>,
            impl Iterator<Item = scolumns::accounts::Key>,
            impl Iterator<Item = (scolumns::dreps::Key, CertificatePointer)>,
            impl Iterator<Item = scolumns::cc_members::Key>,
            impl Iterator<Item = ()>,
            impl Iterator<Item = ()>,
        >,
        withdrawals: impl Iterator<Item = scolumns::accounts::Key>,
    ) -> Result<(), StoreError> {
        match (point, self.tip().ok()) {
            (Point::Specific(new, _), Some(Point::Specific(current, _)))
                if *new <= current && !self.host.incremental_save =>
            {
                trace!(target: EVENT_TARGET, ?point, "save.point_already_known");
            }
            _ => {
                let tip = point.slot_or_default();
                self.db
                    .put(KEY_TIP, as_value(point))
                    .map_err(|err| StoreError::Internal(err.into()))?;

                let current_epoch = era_history
                    .slot_to_epoch(tip, tip)
                    .map_err(|err| StoreError::Internal(err.into()))?;

                if let Some(issuer) = issuer {
                    slots::put(&self.db, &tip, scolumns::slots::Row::new(*issuer))?;
                }

                let drep_validity = current_epoch + protocol_parameters.drep_expiry
                    - governance_activity.consecutive_dormant_epochs as u64;

                utxo::add(&self.db, add.utxo)?;
                pools::add(&self.db, add.pools)?;
                dreps::add(&self.db, drep_validity, add.dreps)?;
                accounts::add(&self.db, add.accounts)?;
                cc_members::add(&self.db, add.cc_members)?;

                let proposals_count = proposals::add(&self.db, add.proposals)?;
                let voting_dreps = votes::add(&self.db, add.votes)?;

                // Reset validity period of voting dreps.
                if !voting_dreps.is_empty() {
                    dreps::set_valid_until(&self.db, voting_dreps, drep_validity)?;
                }

                accounts::reset_many(&self.db, withdrawals)?;

                utxo::remove(&self.db, remove.utxo)?;
                pools::remove(&self.db, remove.pools)?;
                accounts::remove(&self.db, remove.accounts)?;
                dreps::remove(&self.db, remove.dreps)?;

                // When a proposal is seen during a dormant period, we flush the current dormant
                // epochs counter on each drep.
                if governance_activity.consecutive_dormant_epochs > 0 && proposals_count > 0 {
                    self.with_dreps(|iterator| {
                        for (_, mut entry) in iterator {
                            if let Some(row) = entry.borrow_mut() {
                                let actual_expiry = row.valid_until
                                    + governance_activity.consecutive_dormant_epochs as u64;
                                if actual_expiry >= current_epoch {
                                    row.valid_until = actual_expiry;
                                }
                            }
                        }
                    })?;

                    governance_activity.consecutive_dormant_epochs = 0;

                    self.set_governance_activity(governance_activity)?;
                }
            }
        }
        Ok(())
    }

    fn with_pots<'db>(
        &self,
        mut with: impl FnMut(Box<dyn std::borrow::BorrowMut<scolumns::pots::Row> + '_>),
    ) -> Result<(), StoreError> {
        let mut err = None;
        let proxy = Box::new(BorrowableProxy::new(
            pots::get(|key| self.db.get(key))?,
            |pots| {
                let put = pots::put(&self.db, pots);
                if let Err(e) = put {
                    err = Some(e);
                }
            },
        ));

        with(proxy);

        match err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    fn with_utxo(&self, with: impl FnMut(scolumns::utxo::Iter<'_, '_>)) -> Result<(), StoreError> {
        with_prefix_iterator(&self.db, utxo::PREFIX, with)
    }

    fn with_pools(
        &self,
        with: impl FnMut(scolumns::pools::Iter<'_, '_>),
    ) -> Result<(), StoreError> {
        with_prefix_iterator(&self.db, pools::PREFIX, with)
    }

    fn with_accounts(
        &self,
        with: impl FnMut(scolumns::accounts::Iter<'_, '_>),
    ) -> Result<(), StoreError> {
        with_prefix_iterator(&self.db, accounts::PREFIX, with)
    }

    fn with_block_issuers(
        &self,
        with: impl FnMut(scolumns::slots::Iter<'_, '_>),
    ) -> Result<(), StoreError> {
        with_prefix_iterator(&self.db, slots::PREFIX, with)
    }

    fn with_dreps(
        &self,
        with: impl FnMut(scolumns::dreps::Iter<'_, '_>),
    ) -> Result<(), StoreError> {
        with_prefix_iterator(&self.db, dreps::PREFIX, with)
    }

    fn with_proposals(
        &self,
        with: impl FnMut(scolumns::proposals::Iter<'_, '_>),
    ) -> Result<(), StoreError> {
        with_prefix_iterator(&self.db, proposals::PREFIX, with)
    }

    fn with_cc_members(
        &self,
        with: impl FnMut(scolumns::cc_members::Iter<'_, '_>),
    ) -> Result<(), StoreError> {
        with_prefix_iterator(&self.db, cc_members::PREFIX, with)
    }
}

// Helpers
// ----------------------------------------------------------------------------

/// Splits a vector of numbers into groups of continuous numbers.
/// e.g. `[1, 2, 3, 5, 6, 8]` becomes `[[1, 2, 3], [5, 6], [8]]`.
fn split_continuous(input: Vec<u64>) -> Vec<Vec<u64>> {
    input
        .into_iter()
        .fold(vec![], |mut acc, x| match acc.last() {
            Some(last_group) if last_group.last().is_some_and(|&last| x == last + 1) => {
                let mut new_acc = acc[..acc.len() - 1].to_vec();
                let mut new_group = last_group.clone();
                new_group.push(x);
                new_acc.push(new_group);
                new_acc
            }
            _ => {
                acc.push(vec![x]);
                acc
            }
        })
}

fn pretty_print_snapshot_ranges(ranges: &[Vec<u64>]) -> String {
    ranges
        .iter()
        .map(|g| match g.len() {
            0 => "[]".to_string(),
            1 => format!("[{}]", g[0]),
            #[allow(clippy::unwrap_used)] // Infallible error.
            _ => format!("[{}..{}]", g.first().unwrap(), g.last().unwrap()),
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn assert_sufficient_snapshots(dir: &Path) -> Result<(), StoreError> {
    let snapshots = RocksDB::snapshots(dir)?;
    let snapshots_ranges = split_continuous(snapshots.iter().map(|e| u64::from(*e)).collect());
    info!(target: EVENT_TARGET, snapshots = pretty_print_snapshot_ranges(&snapshots_ranges), "new.known_snapshots");
    if snapshots_ranges.len() != 1 && snapshots_ranges[0].len() < 2 {
        return Err(StoreError::Open(OpenErrorKind::NoStableSnapshot));
    }
    Ok(())
}

fn default_opts_with_prefix() -> Options {
    let mut opts = Options::default();
    opts.set_prefix_extractor(SliceTransform::create_fixed_prefix(PREFIX_LEN));
    opts
}

fn get_or_bail<T>(
    db_get: impl Fn(&str) -> Result<Option<Vec<u8>>, rocksdb::Error>,
    key: &str,
) -> Result<T, StoreError>
where
    T: std::fmt::Debug + for<'d> cbor::decode::Decode<'d, ()> + 'static,
{
    get(db_get, key)?.ok_or(StoreError::missing::<T>(key))
}

fn get<T>(
    db_get: impl Fn(&str) -> Result<Option<Vec<u8>>, rocksdb::Error>,
    key: &str,
) -> Result<Option<T>, StoreError>
where
    T: std::fmt::Debug + for<'d> cbor::decode::Decode<'d, ()> + 'static,
{
    (db_get)(key)
        .map_err(|err| StoreError::Internal(err.into()))?
        .map(|b| cbor::decode(b.as_ref()))
        .transpose()
        .map_err(StoreError::Undecodable)
}

#[allow(clippy::panic)]
#[allow(clippy::unwrap_used)]
pub fn iter<'a, 'b, K, V, DB, F>(
    db_iter_opt: F,
    prefix: [u8; PREFIX_LEN],
    direction: Direction,
) -> Result<impl Iterator<Item = (K, V)> + 'a, StoreError>
where
    DB: 'a + 'b + DBAccess,
    F: Fn(IteratorMode<'_>, ReadOptions) -> DBIteratorWithThreadMode<'b, DB> + 'a,
    'b: 'a,
    K: for<'d> cbor::Decode<'d, ()> + 'a,
    V: for<'d> cbor::Decode<'d, ()> + 'a,
{
    let mut opts = ReadOptions::default();
    opts.set_prefix_same_as_start(true);
    let it = (db_iter_opt)(IteratorMode::From(prefix.as_ref(), direction), opts);
    let decoded_it = it.map(|e| {
        let (key, value) = e.unwrap();
        let k = cbor::decode(&key[PREFIX_LEN..]).unwrap_or_else(|e| {
            panic!(
                "unable to decode key {}::<{}> for type {}: {e:?}",
                hex::encode(&key),
                std::any::type_name::<K>(),
                std::any::type_name::<V>()
            )
        });
        let v = cbor::decode(&value).unwrap_or_else(|e| {
            panic!(
                "unable to decode value {}::<{}> for key {}::<{}>: {e:?}",
                hex::encode(&value),
                std::any::type_name::<V>(),
                hex::encode(&key),
                std::any::type_name::<K>(),
            )
        });
        (k, v)
    });
    Ok(decoded_it)
}

/// An generic column iterator, provided that rows from the column are (de)serialisable.
#[allow(clippy::panic)]
fn with_prefix_iterator<
    K: Clone + fmt::Debug + for<'d> cbor::Decode<'d, ()> + cbor::Encode<()>,
    V: Clone + fmt::Debug + for<'d> cbor::Decode<'d, ()> + cbor::Encode<()>,
    DB,
>(
    db: &rocksdb::Transaction<'_, DB>,
    prefix: [u8; PREFIX_LEN],
    mut with: impl FnMut(IterBorrow<'_, '_, K, Option<V>>),
) -> Result<(), StoreError> {
    let mut iterator =
        amaru_iter_borrow::new::<PREFIX_LEN, _, _>(db.prefix_iterator(prefix).map(|item| {
            // FIXME: clarify what kind of errors can come from the database at this point.
            // We are merely iterating over a collection.
            item.unwrap_or_else(|e| panic!("unexpected database error: {e:?}"))
        }));

    with(iterator.as_iter_borrow());

    for (k, v) in iterator.into_iter_updates() {
        match v {
            Some(v) => db.put(k, as_value(v)),
            None => db.delete(k),
        }
        .map_err(|err| StoreError::Internal(err.into()))?;
    }
    Ok(())
}

// Tests
// ----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use amaru_kernel::{network::NetworkName, EraHistory};
    use proptest::test_runner::TestRunner;
    use tempfile::TempDir;

    use crate::{
        rocksdb::{pretty_print_snapshot_ranges, split_continuous, ReadOnlyRocksDB, RocksDB},
        tests::{
            add_test_data_to_store, test_epoch_transition, test_read_account, test_read_drep,
            test_read_pool, test_read_utxo, test_refund_account, test_remove_account,
            test_remove_drep, test_remove_pool, test_remove_utxo, test_slot_updated, Fixture,
        },
    };
    use amaru_ledger::store::StoreError;

    #[cfg(not(target_os = "windows"))]
    use crate::tests::test_read_proposal;

    fn setup_rocksdb_store(runner: &mut TestRunner) -> Result<(RocksDB, Fixture), StoreError> {
        let era_history: EraHistory =
            (*Into::<&'static EraHistory>::into(NetworkName::Preprod)).clone();
        let tmp_dir = TempDir::new().expect("failed to create temp dir");

        let store = RocksDB::empty(tmp_dir.path()).map_err(|e| StoreError::Internal(e.into()))?;

        let fixture = add_test_data_to_store(&store, &era_history, runner)?;
        Ok((store, fixture))
    }

    #[test]
    fn open_one_writer_and_one_reader() {
        use std::fs::File;
        use tempfile::TempDir;

        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("0");
        let _fake_snapshot = File::create(&file_path).unwrap();

        let rw_db = RocksDB::new(dir.path()).inspect_err(|e| eprintln!("{e:#?}"));
        assert!(matches!(rw_db, Ok(..)));

        let ro_db = ReadOnlyRocksDB::new(dir.path()).inspect_err(|e| eprintln!("{e:#?}"));
        assert!(matches!(ro_db, Ok(..)));
    }

    #[test]
    fn test_rocksdb_read_account() {
        let mut runner = TestRunner::default();
        let (store, fixture) = setup_rocksdb_store(&mut runner).expect("Failed to setup store");
        test_read_account(&store, &fixture);
    }

    #[test]
    fn test_rocksdb_read_pool() {
        let mut runner = TestRunner::default();
        let (store, fixture) = setup_rocksdb_store(&mut runner).expect("Failed to setup store");
        test_read_pool(&store, &fixture);
    }

    #[test]
    fn test_rocksdb_read_drep() {
        let mut runner = TestRunner::default();
        let (store, fixture) = setup_rocksdb_store(&mut runner).expect("Failed to setup store");
        test_read_drep(&store, &fixture);
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn test_rocksdb_read_proposal() {
        let mut runner = TestRunner::default();
        let (store, fixture) = setup_rocksdb_store(&mut runner).expect("Failed to setup store");
        test_read_proposal(&store, &fixture);
    }

    #[test]
    fn test_rocksdb_refund_account() -> Result<(), StoreError> {
        let mut runner = TestRunner::default();
        let (store, fixture) = setup_rocksdb_store(&mut runner)?;
        test_refund_account(&store, &fixture, &mut runner)
    }

    #[test]
    fn test_rocksdb_epoch_transition() -> Result<(), StoreError> {
        let mut runner = TestRunner::default();
        let (store, _) = setup_rocksdb_store(&mut runner)?;
        test_epoch_transition(&store)
    }

    #[test]
    fn test_rocksdb_slot_updated() -> Result<(), StoreError> {
        let mut runner = TestRunner::default();
        let (store, fixture) = setup_rocksdb_store(&mut runner)?;
        test_slot_updated(&store, &fixture)
    }

    #[test]
    fn test_rocksdb_read_utxo() {
        let mut runner = TestRunner::default();
        let (store, fixture) = setup_rocksdb_store(&mut runner).expect("Failed to setup store");
        test_read_utxo(&store, &fixture);
    }
    #[test]
    fn test_rocksdb_remove_utxo() -> Result<(), StoreError> {
        let mut runner = TestRunner::default();
        let (store, fixture) = setup_rocksdb_store(&mut runner)?;
        test_remove_utxo(&store, &fixture)
    }

    #[test]
    fn test_rocksdb_remove_account() -> Result<(), StoreError> {
        let mut runner = TestRunner::default();
        let (store, fixture) = setup_rocksdb_store(&mut runner)?;
        test_remove_account(&store, &fixture)
    }

    #[test]
    fn test_rocksdb_remove_pool() -> Result<(), StoreError> {
        let mut runner = TestRunner::default();
        let (store, fixture) = setup_rocksdb_store(&mut runner)?;
        test_remove_pool(&store, &fixture)
    }

    #[test]
    fn test_rocksdb_remove_drep() -> Result<(), StoreError> {
        let mut runner = TestRunner::default();
        let (store, fixture) = setup_rocksdb_store(&mut runner)?;
        test_remove_drep(&store, &fixture)
    }

    #[test]
    #[ignore]
    fn test_rocksdb_iterate_cc_members() {
        unimplemented!()
    }

    #[test]
    #[ignore]
    fn test_rocksdb_remove_cc_members() {
        unimplemented!()
    }

    #[test]
    fn split_all_continuous() {
        let input = vec![1, 2, 3, 4, 5];
        let expected = vec![vec![1, 2, 3, 4, 5]];
        assert_eq!(split_continuous(input), expected);
    }

    #[test]
    fn split_mixed_groups() {
        let input = vec![1, 2, 3, 5, 6, 10];
        let expected = vec![vec![1, 2, 3], vec![5, 6], vec![10]];
        assert_eq!(split_continuous(input), expected);
    }

    #[test]
    fn pp_all_continuous() {
        let input = vec![vec![1, 2, 3]];
        let expected = "[1..3]".to_string();
        assert_eq!(pretty_print_snapshot_ranges(&input), expected);
    }

    #[test]
    fn pp_mixed_groups() {
        let input = vec![vec![1, 2, 3], vec![5], vec![7, 8]];
        let expected = "[1..3],[5],[7..8]".to_string();
        assert_eq!(pretty_print_snapshot_ranges(&input), expected);
    }
}
