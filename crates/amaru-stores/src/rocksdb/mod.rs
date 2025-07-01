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
use amaru_kernel::{
    protocol_parameters::ProtocolParameters, CertificatePointer, EraHistory, Lovelace, Point,
    PoolId, StakeCredential, TransactionInput, TransactionOutput,
};
use amaru_ledger::{
    store::{
        columns as scolumns, Columns, EpochTransitionProgress, HistoricalStores, OpenErrorKind,
        ReadOnlyStore, Snapshot, Store, StoreError, TipErrorKind, TransactionalContext,
    },
    summary::Pots,
};
use iter_borrow::{self, borrowable_proxy::BorrowableProxy, IterBorrow};
use pallas_codec::minicbor::{self as cbor};
use rocksdb::{
    DBAccess, DBIteratorWithThreadMode, Direction, IteratorMode, ReadOptions, Transaction, DB,
};
use slot_arithmetic::Epoch;
use std::{
    collections::BTreeSet,
    fmt, fs,
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

const EVENT_TARGET: &str = "amaru::ledger::store";

/// Special key where we store the tip of the database (most recently applied delta)
const KEY_TIP: &str = "tip";

/// Special key where we store the progress of the database
const KEY_PROGRESS: &str = "progress";

// Special key where we store the protocol parameters
const PROTOCOL_PARAMETERS_PREFIX: &str = "ppar";

/// Name of the directory containing the live ledger stable database.
const DIR_LIVE_DB: &str = "live";

/// An opaque handle for a store implementation of top of RocksDB. The database has the
/// following structure:
///
/// * ========================*=============================================== *
/// * key                     * value                                          *
/// * ========================*=============================================== *
/// * 'tip'                   * Point                                          *
/// * 'progress'              * EpochTransitionProgress                        *
/// * 'pots'                  * (Lovelace, Lovelace, Lovelace)                 *
/// * 'utxo:'TransactionInput * TransactionOutput                              *
/// * 'pool:'PoolId           * (PoolParams, Vec<(Option<PoolParams>, Epoch)>) *
/// * 'acct:'StakeCredential  * (Option<PoolId>, Lovelace, Lovelace)           *
/// * 'slot':slot             * PoolId                                         *
/// * ========================*=============================================== *
///
/// CBOR is used to serialize objects (as keys or values) into their binary equivalent.
pub struct RocksDB {
    /// The working directory where we store the various key/value stores.
    dir: PathBuf,

    /// Whether to allow saving data incrementally (i.e. calling multiple times 'save' with the
    /// same point). This is only sound when importing static data, but not when running live.
    incremental_save: bool,

    /// An instance of RocksDB.
    db: OptimisticTransactionDB,

    /// The `EraHistory` of the network this database is tied to
    era_history: EraHistory,

    ongoing_transaction: OngoingTransaction,
}

fn validate_snapshots(dir: &Path) -> Result<(), StoreError> {
    let snapshots = RocksDB::snapshots(dir)?;
    info!(target: EVENT_TARGET, snapshots = ?snapshots, "new.known_snapshots");
    if snapshots.is_empty() {
        return Err(StoreError::Open(OpenErrorKind::NoStableSnapshot));
    }
    Ok(())
}

fn default_opts_with_prefix() -> Options {
    let mut opts = Options::default();
    opts.set_prefix_extractor(SliceTransform::create_fixed_prefix(PREFIX_LEN));
    opts
}

pub struct ReadOnlyLedgerDB {
    db: DB,
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

    pub fn new(dir: &Path, era_history: &EraHistory) -> Result<RocksDB, StoreError> {
        validate_snapshots(dir)?;
        let mut opts = default_opts_with_prefix();
        opts.create_if_missing(true);
        OptimisticTransactionDB::open(&opts, dir.join("live"))
            .map(|db| Self {
                dir: dir.to_path_buf(),
                incremental_save: false,
                db,
                era_history: era_history.clone(),
                ongoing_transaction: OngoingTransaction::new(),
            })
            .map_err(|err| StoreError::Internal(err.into()))
    }

    pub fn open_for_read_only(dir: &Path) -> Result<ReadOnlyLedgerDB, StoreError> {
        validate_snapshots(dir)?;
        let opts = default_opts_with_prefix();
        rocksdb::DB::open_for_read_only(&opts, dir.join("live"), false)
            .map(|db| ReadOnlyLedgerDB { db })
            .map_err(|err| StoreError::Internal(err.into()))
    }

    pub fn empty(dir: &Path, era_history: &EraHistory) -> Result<RocksDB, StoreError> {
        let mut opts = Options::default();
        opts.create_if_missing(true);
        opts.set_prefix_extractor(SliceTransform::create_fixed_prefix(PREFIX_LEN));
        OptimisticTransactionDB::open(&opts, dir.join("live"))
            .map(|db| Self {
                dir: dir.to_path_buf(),
                incremental_save: true,
                db,
                era_history: era_history.clone(),
                ongoing_transaction: OngoingTransaction::new(),
            })
            .map_err(|err| StoreError::Internal(err.into()))
    }

    fn transaction_ended(&self) {
        self.ongoing_transaction.set(false);
    }
}

fn get<T: for<'d> cbor::decode::Decode<'d, ()>>(
    db_get: impl Fn(&str) -> Result<Option<Vec<u8>>, rocksdb::Error>,
    key: &str,
) -> Result<Option<T>, StoreError> {
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
        let k = cbor::decode(&key[PREFIX_LEN..])
            .unwrap_or_else(|e| panic!("unable to decode key ({}): {e:?}", hex::encode(&key)));
        let v = cbor::decode(&value).unwrap_or_else(|e| {
            panic!(
                "unable to decode value ({}) for key ({}): {e:?}",
                hex::encode(&value),
                hex::encode(&key)
            )
        });
        (k, v)
    });
    Ok(decoded_it)
}

macro_rules! impl_ReadOnlyStore {
    (for $($s:ty),+) => {
        $(impl ReadOnlyStore for $s {
            fn get_protocol_parameters_for(
                &self,
                epoch: &Epoch,
            ) -> Result<ProtocolParameters, StoreError> {
                get(|key| self.db.get(key), &format!("{PROTOCOL_PARAMETERS_PREFIX}:{epoch}"))
                    .map(|row| row.unwrap_or_default())
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

            fn utxo(&self, input: &TransactionInput) -> Result<Option<TransactionOutput>, StoreError> {
                utxo::get(|key| self.db.get(key), input)
            }

            fn iter_utxos(
                &self,
            ) -> Result<impl Iterator<Item = (scolumns::utxo::Key, scolumns::utxo::Value)>, StoreError>
            {
                iter(|mode, opts| self.db.iterator_opt(mode, opts), utxo::PREFIX, Direction::Forward)
            }

            fn pots(&self) -> Result<Pots, StoreError> {
                pots::get(|key| self.db.get(key)).map(|row| Pots::from(&row))
            }

            fn iter_accounts(
                &self,
            ) -> Result<impl Iterator<Item = (scolumns::accounts::Key, scolumns::accounts::Row)>, StoreError>
            {
                iter(|mode, opts| self.db.iterator_opt(mode, opts), accounts::PREFIX, Direction::Forward)
            }

            fn iter_block_issuers(
                &self,
            ) -> Result<impl Iterator<Item = (scolumns::slots::Key, scolumns::slots::Value)>, StoreError>
            {
                iter(|mode, opts| self.db.iterator_opt(mode, opts), slots::PREFIX, Direction::Forward)
            }

            fn iter_pools(
                &self,
            ) -> Result<impl Iterator<Item = (scolumns::pools::Key, scolumns::pools::Row)>, StoreError>
            {
                iter(|mode, opts| self.db.iterator_opt(mode, opts), pools::PREFIX, Direction::Forward)
            }

            fn iter_dreps(
                &self,
            ) -> Result<impl Iterator<Item = (scolumns::dreps::Key, scolumns::dreps::Row)>, StoreError>
            {
                iter(|mode, opts| self.db.iterator_opt(mode, opts), dreps::PREFIX, Direction::Forward)
            }

            fn iter_proposals(
                &self,
            ) -> Result<
                impl Iterator<Item = (scolumns::proposals::Key, scolumns::proposals::Row)>,
                StoreError,
            > {
                iter(|mode, opts| self.db.iterator_opt(mode, opts), proposals::PREFIX, Direction::Forward)
            }
        })*
    }
}

// For now RocksDB and RocksDBSnapshot share their implementation of ReadOnlyStore
impl_ReadOnlyStore!(for RocksDB, RocksDBSnapshot, ReadOnlyLedgerDB, ReadOnlySnapshotLedgerDB);

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
        iter_borrow::new::<PREFIX_LEN, _, _>(db.prefix_iterator(prefix).map(|item| {
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

struct RocksDBTransactionalContext<'a> {
    db: &'a RocksDB,
    transaction: Transaction<'a, OptimisticTransactionDB>,
}

impl TransactionalContext<'_> for RocksDBTransactionalContext<'_> {
    fn commit(self) -> Result<(), StoreError> {
        let res = self
            .transaction
            .commit()
            .map_err(|err| StoreError::Internal(err.into()));
        self.db.transaction_ended();
        res
    }

    fn rollback(mut self) -> Result<(), StoreError> {
        let transaction = std::mem::replace(&mut self.transaction, self.db.db.transaction());
        let res = transaction
            .rollback()
            .map_err(|err| StoreError::Internal(err.into()));
        self.db.transaction_ended();
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
            .transaction
            .get(KEY_PROGRESS)
            .map_err(|err| StoreError::Internal(err.into()))?
            .map(|bytes| cbor::decode(&bytes))
            .transpose()
            .map_err(StoreError::Undecodable)?;

        if previous_progress != from {
            return Ok(false);
        }

        match to {
            None => self.transaction.delete(KEY_PROGRESS),
            Some(to) => self.transaction.put(KEY_PROGRESS, as_value(to)),
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
        accounts::set(&self.transaction, credential, |balance| balance + deposit)
    }

    fn set_protocol_parameters(
        &self,
        epoch: &Epoch,
        protocol_parameters: &ProtocolParameters,
    ) -> Result<(), StoreError> {
        self.transaction
            .put(
                format!("{PROTOCOL_PARAMETERS_PREFIX}:{epoch}"),
                as_value(protocol_parameters),
            )
            .map_err(|err| StoreError::Internal(err.into()))?;

        Ok(())
    }

    fn save(
        &self,
        point: &Point,
        issuer: Option<&scolumns::pools::Key>,
        add: Columns<
            impl Iterator<Item = (scolumns::utxo::Key, scolumns::utxo::Value)>,
            impl Iterator<Item = scolumns::pools::Value>,
            impl Iterator<Item = (scolumns::accounts::Key, scolumns::accounts::Value)>,
            impl Iterator<Item = (scolumns::dreps::Key, scolumns::dreps::Value)>,
            impl Iterator<Item = (scolumns::cc_members::Key, scolumns::cc_members::Value)>,
            impl Iterator<Item = (scolumns::proposals::Key, scolumns::proposals::Value)>,
        >,
        remove: Columns<
            impl Iterator<Item = scolumns::utxo::Key>,
            impl Iterator<Item = (scolumns::pools::Key, Epoch)>,
            impl Iterator<Item = scolumns::accounts::Key>,
            impl Iterator<Item = (scolumns::dreps::Key, CertificatePointer)>,
            impl Iterator<Item = scolumns::cc_members::Key>,
            impl Iterator<Item = scolumns::proposals::Key>,
        >,
        withdrawals: impl Iterator<Item = scolumns::accounts::Key>,
        voting_dreps: BTreeSet<StakeCredential>,
    ) -> Result<(), StoreError> {
        match (point, self.db.tip().ok()) {
            (Point::Specific(new, _), Some(Point::Specific(current, _)))
                if *new <= current && !self.db.incremental_save =>
            {
                trace!(target: EVENT_TARGET, ?point, "save.point_already_known");
            }
            _ => {
                self.transaction
                    .put(KEY_TIP, as_value(point))
                    .map_err(|err| StoreError::Internal(err.into()))?;

                if let Some(issuer) = issuer {
                    slots::put(
                        &self.transaction,
                        &point.slot_or_default(),
                        scolumns::slots::Row::new(*issuer),
                    )?;
                }

                utxo::add(&self.transaction, add.utxo)?;
                pools::add(&self.transaction, add.pools)?;
                dreps::add(&self.transaction, add.dreps)?;
                accounts::add(&self.transaction, add.accounts)?;
                cc_members::add(&self.transaction, add.cc_members)?;
                proposals::add(&self.transaction, add.proposals)?;

                accounts::reset_many(&self.transaction, withdrawals)?;
                dreps::tick(&self.transaction, voting_dreps, {
                    let slot = point.slot_or_default();
                    self.db
                        .era_history
                        .slot_to_epoch(slot)
                        .map_err(|err| StoreError::Internal(err.into()))?
                })?;

                utxo::remove(&self.transaction, remove.utxo)?;
                pools::remove(&self.transaction, remove.pools)?;
                accounts::remove(&self.transaction, remove.accounts)?;
                dreps::remove(&self.transaction, remove.dreps)?;
                proposals::remove(&self.transaction, remove.proposals)?;
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
            pots::get(|key| self.transaction.get(key))?,
            |pots| {
                let put = pots::put(&self.transaction, pots);
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
        with_prefix_iterator(&self.transaction, utxo::PREFIX, with)
    }

    fn with_pools(
        &self,
        with: impl FnMut(scolumns::pools::Iter<'_, '_>),
    ) -> Result<(), StoreError> {
        with_prefix_iterator(&self.transaction, pools::PREFIX, with)
    }

    fn with_accounts(
        &self,
        with: impl FnMut(scolumns::accounts::Iter<'_, '_>),
    ) -> Result<(), StoreError> {
        with_prefix_iterator(&self.transaction, accounts::PREFIX, with)
    }

    fn with_block_issuers(
        &self,
        with: impl FnMut(scolumns::slots::Iter<'_, '_>),
    ) -> Result<(), StoreError> {
        with_prefix_iterator(&self.transaction, slots::PREFIX, with)
    }

    fn with_dreps(
        &self,
        with: impl FnMut(scolumns::dreps::Iter<'_, '_>),
    ) -> Result<(), StoreError> {
        with_prefix_iterator(&self.transaction, dreps::PREFIX, with)
    }

    fn with_proposals(
        &self,
        with: impl FnMut(scolumns::proposals::Iter<'_, '_>),
    ) -> Result<(), StoreError> {
        with_prefix_iterator(&self.transaction, proposals::PREFIX, with)
    }
}

impl Store for RocksDB {
    fn snapshots(&self) -> Result<Vec<Epoch>, StoreError> {
        RocksDB::snapshots(&self.dir)
    }

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
    fn create_transaction(&self) -> impl TransactionalContext<'_> {
        if self.ongoing_transaction.get() {
            // Thats a bug in the code, we should never have two transactions at the same time
            panic!("RocksDB already has an ongoing transaction");
        }
        let transaction = self.db.transaction();
        self.ongoing_transaction.set(true);
        RocksDBTransactionalContext {
            transaction,
            db: self,
        }
    }

    fn tip(&self) -> Result<Point, StoreError> {
        get(|key| self.db.get(key), KEY_TIP)?.ok_or(StoreError::Tip(TipErrorKind::Missing))
    }
}

pub struct RocksDBHistoricalStores {
    dir: PathBuf,
}

pub struct ReadOnlySnapshotLedgerDB {
    db: DB,
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

    pub fn open_for_read_only(dir: &Path) -> Result<ReadOnlySnapshotLedgerDB, StoreError> {
        let opts = default_opts_with_prefix();
        rocksdb::DB::open_for_read_only(&opts, dir.join("live"), false)
            .map(|db| ReadOnlySnapshotLedgerDB { db })
            .map_err(|err| StoreError::Internal(err.into()))
    }
}

pub struct RocksDBSnapshot {
    epoch: Epoch,
    db: OptimisticTransactionDB,
}

impl Snapshot for RocksDBSnapshot {
    fn epoch(&'_ self) -> Epoch {
        self.epoch
    }
}

impl HistoricalStores for RocksDBHistoricalStores {
    fn for_epoch(&self, epoch: Epoch) -> Result<impl Snapshot, StoreError> {
        RocksDBHistoricalStores::for_epoch_with(&self.dir, epoch)
    }
}

#[cfg(test)]
#[test]
fn test_two_open_ledger_dbs() {
    use amaru_kernel::network::NetworkName;
    use std::fs::File;
    use tempfile::TempDir;

    let dir = TempDir::new().unwrap();
    let file_path = dir.path().join("0");
    let _snapshot = File::create(&file_path).unwrap();

    let era_history: &EraHistory = NetworkName::Preprod.into();
    let _rw_db = RocksDB::new(dir.path(), era_history).unwrap();

    let _ro_db = RocksDB::open_for_read_only(dir.path()).expect("Unable to open read only db");
}
