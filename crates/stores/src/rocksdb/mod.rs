use ::rocksdb::{self, checkpoint, OptimisticTransactionDB, Options, SliceTransform};
use amaru_ledger::{
    iter::borrow::{self as iter_borrow, borrowable_proxy::BorrowableProxy, IterBorrow},
    kernel::{Epoch, Point, PoolId, TransactionInput, TransactionOutput},
    store::{columns as scolumns, Columns, OpenErrorKind, RewardsSummary, Store, StoreError},
};
use columns::*;
use common::{as_value, PREFIX_LEN};
use pallas_codec::minicbor::{self as cbor};
use std::{
    fmt, fs,
    path::{Path, PathBuf},
};
use tracing::{debug, info, info_span, warn};

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
                debug!(target: EVENT_TARGET, epoch, "new.found_snapshot");
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

impl Store for RocksDB {
    fn for_epoch(&self, epoch: Epoch) -> Result<Box<RocksDB>, StoreError> {
        let mut opts = Options::default();
        opts.set_prefix_extractor(SliceTransform::create_fixed_prefix(PREFIX_LEN));

        Ok(Box::new(RocksDB {
            snapshots: vec![epoch],
            dir: self.dir.to_path_buf(),
            db: OptimisticTransactionDB::open(
                &opts,
                self.dir.join(PathBuf::from(format!("{epoch:?}"))),
            )
            .map_err(|err| StoreError::Internal(err.into()))?,
        }))
    }

    fn tip(&self) -> Result<Point, StoreError> {
        Ok(self
            .db
            .get(KEY_TIP)
            .map_err(|err| StoreError::Internal(err.into()))?
            .map(|bytes| cbor::decode(&bytes))
            .transpose()
            .expect("unable to decode database's tip")
            .unwrap_or_else(|| {
                panic!("no database tip. Did you forget to 'import' a snapshot first?")
            }))
    }

    fn save(
        &'_ self,
        point: &'_ Point,
        issuer: Option<&'_ scolumns::pools::Key>,
        add: Columns<
            impl Iterator<Item = (scolumns::utxo::Key, scolumns::utxo::Value)>,
            impl Iterator<Item = scolumns::pools::Value>,
            impl Iterator<Item = (scolumns::accounts::Key, scolumns::accounts::Value)>,
        >,
        remove: Columns<
            impl Iterator<Item = scolumns::utxo::Key>,
            impl Iterator<Item = (scolumns::pools::Key, Epoch)>,
            impl Iterator<Item = scolumns::accounts::Key>,
        >,
    ) -> Result<(), StoreError> {
        let batch = self.db.transaction();

        let tip: Option<Point> = batch
            .get(KEY_TIP)
            .map_err(|err| StoreError::Internal(err.into()))?
            .map(|bytes| {
                cbor::decode(&bytes).unwrap_or_else(|e| {
                    panic!(
                        "unable to decode database tip ({}): {e:?}",
                        hex::encode(&bytes)
                    )
                })
            });

        match (point, tip) {
            (Point::Specific(new, _), Some(Point::Specific(current, _))) if *new <= current => {
                info!(target: EVENT_TARGET, ?point, "save.point_already_known");
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

                utxo::remove(&batch, remove.utxo)?;
                pools::remove(&batch, remove.pools)?;
                accounts::remove(&batch, remove.accounts)?;
            }
        }

        batch
            .commit()
            .map_err(|err| StoreError::Internal(err.into()))
    }

    fn most_recent_snapshot(&'_ self) -> Epoch {
        self.snapshots
            .last()
            .cloned()
            .unwrap_or_else(|| panic!("called 'most_recent_snapshot' on empty database?!"))
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

                info_span!(target: EVENT_TARGET, "snapshot.adjusting_pots", delta_treasury, delta_reserves, unclaimed_rewards).in_scope(|| {
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
                fs::remove_dir_all(&path).expect("Unable to remove existing snapshot directory");
            }
            checkpoint::Checkpoint::new(&self.db)
                .map_err(|err| StoreError::Internal(err.into()))?
                .create_checkpoint(path)
                .map_err(|err| StoreError::Internal(err.into()))?;

            info_span!(target: EVENT_TARGET, "reset.blocks_count").in_scope(|| {
                // TODO: If necessary, come up with a more efficient way of dropping a "table".
                // RocksDB does support batch-removing of key ranges, but somehow, not in a
                // transactional way. So it isn't as trivial to implement as it may seem.
                self.with_block_issuers(|iterator| {
                    for (_, mut row) in iterator {
                        *row.borrow_mut() = None;
                    }
                })
            })?;

            info_span!(target: EVENT_TARGET, "reset.fees").in_scope(|| {
                self.with_pots(|mut row| {
                    row.borrow_mut().fees = 0;
                })
            })?;

            self.snapshots.push(snapshot);
        } else {
            info!(target: EVENT_TARGET, %epoch, "next_snapshot.already_known");
        }

        Ok(())
    }

    fn with_pots<A>(
        &self,
        mut with: impl FnMut(Box<dyn std::borrow::BorrowMut<scolumns::pots::Row> + '_>) -> A,
    ) -> Result<A, StoreError> {
        let db = self.db.transaction();

        let mut err = None;
        let proxy = Box::new(BorrowableProxy::new(pots::get(&db)?, |pots| {
            let put = pots::put(&db, pots)
                .and_then(|_| db.commit().map_err(|err| StoreError::Internal(err.into())));
            if let Err(e) = put {
                err = Some(e);
            }
        }));

        let result = with(proxy);

        match err {
            Some(e) => Err(e),
            None => Ok(result),
        }
    }

    fn pool(&self, pool: &PoolId) -> Result<Option<scolumns::pools::Row>, StoreError> {
        pools::get(&self.db, pool)
    }

    fn resolve_input(
        &self,
        input: &TransactionInput,
    ) -> Result<Option<TransactionOutput>, StoreError> {
        utxo::get(&self.db, input)
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

/// An generic column iterator, provided that rows from the column are (de)serialisable.
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
