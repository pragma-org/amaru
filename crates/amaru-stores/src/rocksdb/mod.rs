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
    epoch_from_slot, Epoch, Point, PoolId, StakeCredential, TransactionInput, TransactionOutput,
};
use amaru_ledger::{
    rewards::Pots,
    store::{
        columns as scolumns, Columns, OpenErrorKind, RewardsSummary, Snapshot, Store, StoreError,
        TipErrorKind,
    },
};
use columns::*;
use common::{as_value, PREFIX_LEN};
use iter_borrow::{self, borrowable_proxy::BorrowableProxy, IterBorrow};
use pallas_codec::minicbor::{self as cbor};
use std::{
    collections::BTreeSet,
    fmt, fs,
    path::{Path, PathBuf},
};
use tracing::{info, info_span, trace, trace_span, warn};

pub mod columns;
pub mod common;

const EVENT_TARGET: &str = "amaru::ledger::store";

/// Special key where we store the tip of the database (most recently applied delta)
const KEY_TIP: &str = "tip";

/// Name of the directory containing the live ledger stable database.
const DIR_LIVE_DB: &str = "live";

/// An opaque handle for a store implementation of top of RocksDB. The database has the
/// following structure:
///
/// * ========================*=============================================== *
/// * key                     * value                                          *
/// * ========================*=============================================== *
/// * 'tip'                   * Point                                          *
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

    /// An instance of RocksDB.
    db: OptimisticTransactionDB,

    /// An ordered (asc) list of epochs for which we have available snapshots
    snapshots: Vec<Epoch>,
}

impl RocksDB {
    pub fn new(dir: &Path) -> Result<RocksDB, StoreError> {
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

        info!(target: EVENT_TARGET, snapshots = ?snapshots, "new.known_snapshots");

        let mut opts = Options::default();
        opts.create_if_missing(true);
        opts.set_prefix_extractor(SliceTransform::create_fixed_prefix(PREFIX_LEN));

        if snapshots.is_empty() {
            return Err(StoreError::Open(OpenErrorKind::NoStableSnapshot));
        }

        Ok(RocksDB {
            snapshots,
            dir: dir.to_path_buf(),
            db: OptimisticTransactionDB::open(&opts, dir.join("live"))
                .map_err(|err| StoreError::Internal(err.into()))?,
        })
    }

    pub fn empty(dir: &Path) -> Result<RocksDB, StoreError> {
        let mut opts = Options::default();
        opts.create_if_missing(true);
        opts.set_prefix_extractor(SliceTransform::create_fixed_prefix(PREFIX_LEN));
        Ok(RocksDB {
            snapshots: vec![],
            dir: dir.to_path_buf(),
            db: OptimisticTransactionDB::open(&opts, dir.join("live"))
                .map_err(|err| StoreError::Internal(err.into()))?,
        })
    }

    pub fn unsafe_transaction(&self) -> rocksdb::Transaction<'_, OptimisticTransactionDB> {
        self.db.transaction()
    }
}

#[allow(clippy::panic)]
#[allow(clippy::unwrap_used)]
fn iter<'a, K: Clone + for<'d> cbor::Decode<'d, ()>, V: Clone + for<'d> cbor::Decode<'d, ()>>(
    db: &OptimisticTransactionDB,
    prefix: [u8; PREFIX_LEN],
) -> Result<impl Iterator<Item = (K, V)> + use<'_, K, V>, StoreError> {
    Ok(db.prefix_iterator(prefix).map(|e| {
        let (key, value) = e.unwrap();
        let key = cbor::decode(&key[PREFIX_LEN..])
            .unwrap_or_else(|e| panic!("unable to decode object ({}): {e:?}", hex::encode(&key)));
        let value = cbor::decode(&value)
            .unwrap_or_else(|e| panic!("unable to decode object ({}): {e:?}", hex::encode(&value)));
        (key, value)
    }))
}

impl Snapshot for RocksDB {
    #[allow(clippy::panic)]
    fn most_recent_snapshot(&'_ self) -> Epoch {
        self.snapshots
            .last()
            .cloned()
            .unwrap_or_else(|| panic!("called 'most_recent_snapshot' on empty database?!"))
    }

    fn pool(&self, pool: &PoolId) -> Result<Option<scolumns::pools::Row>, StoreError> {
        pools::get(&self.db, pool)
    }

    fn utxo(&self, input: &TransactionInput) -> Result<Option<TransactionOutput>, StoreError> {
        utxo::get(&self.db, input)
    }

    fn iter_utxos(
        &self,
    ) -> Result<impl Iterator<Item = (scolumns::utxo::Key, scolumns::utxo::Value)>, StoreError>
    {
        iter::<scolumns::utxo::Key, scolumns::utxo::Value>(&self.db, utxo::PREFIX)
    }

    fn pots(&self) -> Result<Pots, StoreError> {
        pots::get(&self.db.transaction()).map(|row| Pots::from(&row))
    }

    fn iter_accounts(
        &self,
    ) -> Result<impl Iterator<Item = (scolumns::accounts::Key, scolumns::accounts::Row)>, StoreError>
    {
        iter::<scolumns::accounts::Key, scolumns::accounts::Row>(&self.db, accounts::PREFIX)
    }

    fn iter_block_issuers(
        &self,
    ) -> Result<impl Iterator<Item = (scolumns::slots::Key, scolumns::slots::Value)>, StoreError>
    {
        iter::<scolumns::slots::Key, scolumns::slots::Value>(&self.db, slots::PREFIX)
    }

    fn iter_pools(
        &self,
    ) -> Result<impl Iterator<Item = (scolumns::pools::Key, scolumns::pools::Row)>, StoreError>
    {
        iter::<scolumns::pools::Key, scolumns::pools::Row>(&self.db, pools::PREFIX)
    }
}

/// An generic column iterator, provided that rows from the column are (de)serialisable.
#[allow(clippy::panic)]
fn with_prefix_iterator<
    K: Clone + fmt::Debug + for<'d> cbor::Decode<'d, ()> + cbor::Encode<()>,
    V: Clone + fmt::Debug + for<'d> cbor::Decode<'d, ()> + cbor::Encode<()>,
    DB,
>(
    db: rocksdb::Transaction<'_, DB>,
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

    db.commit().map_err(|err| StoreError::Internal(err.into()))
}

impl Store for RocksDB {
    fn for_epoch(&self, epoch: Epoch) -> Result<impl Snapshot, StoreError> {
        let mut opts = Options::default();
        opts.set_prefix_extractor(SliceTransform::create_fixed_prefix(PREFIX_LEN));

        Ok(RocksDB {
            snapshots: vec![epoch],
            dir: self.dir.to_path_buf(),
            db: OptimisticTransactionDB::open(
                &opts,
                self.dir.join(PathBuf::from(format!("{epoch:?}"))),
            )
            .map_err(|err| StoreError::Internal(err.into()))?,
        })
    }

    fn tip(&self) -> Result<Point, StoreError> {
        self.db
            .get(KEY_TIP)
            .map_err(|err| StoreError::Internal(err.into()))?
            .map(|bytes| cbor::decode(&bytes))
            .transpose()
            .map_err(|err| StoreError::Tip(TipErrorKind::Undecodable(err)))?
            .ok_or(StoreError::Tip(TipErrorKind::Missing))
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
            impl Iterator<Item = (scolumns::delegations::Key, scolumns::delegations::Value)>,
        >,
        remove: Columns<
            impl Iterator<Item = scolumns::utxo::Key>,
            impl Iterator<Item = (scolumns::pools::Key, Epoch)>,
            impl Iterator<Item = scolumns::accounts::Key>,
            impl Iterator<Item = scolumns::dreps::Key>,
            impl Iterator<Item = scolumns::delegations::Key>,
        >,
        withdrawals: impl Iterator<Item = scolumns::accounts::Key>,
        voting_dreps: BTreeSet<StakeCredential>,
    ) -> Result<(), StoreError> {
        let batch = self.db.transaction();

        let tip: Option<Point> = batch
            .get(KEY_TIP)
            .map_err(|err| StoreError::Internal(err.into()))?
            .map(|bytes| {
                #[allow(clippy::panic)]
                cbor::decode(&bytes).unwrap_or_else(|e| {
                    panic!(
                        "unable to decode database tip ({}): {e:?}",
                        hex::encode(&bytes)
                    )
                })
            });

        match (point, tip) {
            (Point::Specific(new, _), Some(Point::Specific(current, _))) if *new <= current => {
                trace!(target: EVENT_TARGET, ?point, "save.point_already_known");
            }
            _ => {
                batch
                    .put(KEY_TIP, as_value(point))
                    .map_err(|err| StoreError::Internal(err.into()))?;

                if let Some(issuer) = issuer {
                    slots::put(
                        &batch,
                        &point.slot_or_default(),
                        scolumns::slots::Row::new(*issuer),
                    )?;
                }

                utxo::add(&batch, add.utxo)?;
                pools::add(&batch, add.pools)?;
                accounts::add(&batch, add.accounts)?;

                let epoch = epoch_from_slot(point.slot_or_default());
                let dreps = add
                    .dreps
                    // Each new DRep has an associated epoch
                    .map(|(key, value)| (key, value, Some(epoch)))
                    // Voting DReps get their epoch extended
                    .chain(
                        voting_dreps
                            .into_iter()
                            .map(|drep| (drep, (None, None), Some(epoch))),
                    );

                dreps::add(&batch, dreps)?;
                delegations::add(&batch, add.delegations)?;

                accounts::reset(&batch, withdrawals)?;

                utxo::remove(&batch, remove.utxo)?;
                pools::remove(&batch, remove.pools)?;
                accounts::remove(&batch, remove.accounts)?;
                dreps::remove(&batch, remove.dreps)?;
            }
        }

        batch
            .commit()
            .map_err(|err| StoreError::Internal(err.into()))
    }

    fn next_snapshot(
        &'_ mut self,
        epoch: Epoch,
        rewards_summary: Option<RewardsSummary>,
    ) -> Result<(), StoreError> {
        let snapshot = self.snapshots.last().map(|s| s + 1).unwrap_or(epoch);
        if snapshot == epoch {
            if let Some(mut rewards_summary) = rewards_summary {
                info_span!(target: EVENT_TARGET, "snapshot.applying_rewards").in_scope(|| {
                    self.with_accounts(|iterator| {
                        for (account, mut row) in iterator {
                            if let Some(rewards) = rewards_summary.extract_rewards(&account) {
                                if rewards > 0 {
                                    if let Some(account) = row.borrow_mut() {
                                        account.rewards += rewards;
                                    }
                                }
                            }
                        }
                    })
                })?;

                let delta_treasury = rewards_summary.delta_treasury();
                let delta_reserves = rewards_summary.delta_reserves();
                let unclaimed_rewards = rewards_summary.unclaimed_rewards();

                trace_span!(target: EVENT_TARGET, "snapshot.adjusting_pots", delta_treasury, delta_reserves, unclaimed_rewards).in_scope(|| {
                    self.with_pots(|mut row| {
                        let pots = row.borrow_mut();
                        pots.treasury += delta_treasury + unclaimed_rewards;
                        pots.reserves -= delta_reserves;
                    })
                })?;
            }

            let path = self.dir.join(snapshot.to_string());
            if path.exists() {
                // RocksDB error can't be created externally, so panic instead
                // It might be better to come up with a global error type
                fs::remove_dir_all(&path).map_err(|_| {
                    StoreError::Internal("Unable to remove existing snapshot directory".into())
                })?;
            }
            checkpoint::Checkpoint::new(&self.db)
                .map_err(|err| StoreError::Internal(err.into()))?
                .create_checkpoint(path)
                .map_err(|err| StoreError::Internal(err.into()))?;

            trace_span!(target: EVENT_TARGET, "reset.blocks_count").in_scope(|| {
                // TODO: If necessary, come up with a more efficient way of dropping a "table".
                // RocksDB does support batch-removing of key ranges, but somehow, not in a
                // transactional way. So it isn't as trivial to implement as it may seem.
                self.with_block_issuers(|iterator| {
                    for (_, mut row) in iterator {
                        *row.borrow_mut() = None;
                    }
                })
            })?;

            trace_span!(target: EVENT_TARGET, "reset.fees").in_scope(|| {
                self.with_pots(|mut row| {
                    row.borrow_mut().fees = 0;
                })
            })?;

            self.snapshots.push(snapshot);
        } else {
            trace!(target: EVENT_TARGET, %epoch, "next_snapshot.already_known");
        }

        Ok(())
    }

    fn with_pots(
        &self,
        mut with: impl FnMut(Box<dyn std::borrow::BorrowMut<scolumns::pots::Row> + '_>),
    ) -> Result<(), StoreError> {
        let db = self.db.transaction();

        let mut err = None;
        let proxy = Box::new(BorrowableProxy::new(pots::get(&db)?, |pots| {
            let put = pots::put(&db, pots)
                .and_then(|_| db.commit().map_err(|err| StoreError::Internal(err.into())));
            if let Err(e) = put {
                err = Some(e);
            }
        }));

        with(proxy);

        match err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    fn with_utxo(&self, with: impl FnMut(scolumns::utxo::Iter<'_, '_>)) -> Result<(), StoreError> {
        with_prefix_iterator(self.db.transaction(), utxo::PREFIX, with)
    }

    fn with_pools(
        &self,
        with: impl FnMut(scolumns::pools::Iter<'_, '_>),
    ) -> Result<(), StoreError> {
        with_prefix_iterator(self.db.transaction(), pools::PREFIX, with)
    }

    fn with_accounts(
        &self,
        with: impl FnMut(scolumns::accounts::Iter<'_, '_>),
    ) -> Result<(), StoreError> {
        with_prefix_iterator(self.db.transaction(), accounts::PREFIX, with)
    }

    fn with_block_issuers(
        &self,
        with: impl FnMut(scolumns::slots::Iter<'_, '_>),
    ) -> Result<(), StoreError> {
        with_prefix_iterator(self.db.transaction(), slots::PREFIX, with)
    }
}
