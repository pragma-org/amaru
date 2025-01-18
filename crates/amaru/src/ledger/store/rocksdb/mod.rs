pub(crate) mod common;

use crate::{
    iter::{
        borrow as iter_borrow,
        borrow::{borrowable_proxy::BorrowableProxy, IterBorrow},
    },
    ledger::{
        kernel::{Epoch, Point, PoolId, TransactionInput, TransactionOutput},
        rewards::StakeDistributionSnapshot,
        store::{columns::*, Columns, RewardsSummary, Store},
    },
};
use ::rocksdb::{self, checkpoint, OptimisticTransactionDB, Options, SliceTransform};
use common::{as_value, PREFIX_LEN};
use miette::Diagnostic;
use pallas_codec::minicbor::{self as cbor};
use std::{
    fmt, fs, io,
    path::{Path, PathBuf},
};
use tracing::{debug, info, info_span, warn};

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

#[derive(Debug, thiserror::Error, Diagnostic)]
pub enum OpenError {
    #[error(transparent)]
    RocksDB(rocksdb::Error),
    #[error(transparent)]
    IO(io::Error),
    #[error("no ledger stable snapshot found in ledger.db; at least one is expected")]
    NoStableSnapshot,
}

impl RocksDB {
    pub fn new(dir: &Path) -> Result<RocksDB, OpenError> {
        let mut snapshots: Vec<Epoch> = Vec::new();
        for entry in fs::read_dir(dir).map_err(OpenError::IO)?.by_ref() {
            let entry = entry.map_err(OpenError::IO)?;

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
            return Err(OpenError::NoStableSnapshot);
        }

        Ok(RocksDB {
            snapshots,
            dir: dir.to_path_buf(),
            db: OptimisticTransactionDB::open(&opts, dir.join("live"))
                .map_err(OpenError::RocksDB)?,
        })
    }

    pub fn empty(dir: &Path) -> Result<RocksDB, OpenError> {
        let mut opts = Options::default();
        opts.create_if_missing(true);
        opts.set_prefix_extractor(SliceTransform::create_fixed_prefix(PREFIX_LEN));
        Ok(RocksDB {
            snapshots: vec![],
            dir: dir.to_path_buf(),
            db: OptimisticTransactionDB::open(&opts, dir.join("live"))
                .map_err(OpenError::RocksDB)?,
        })
    }

    pub fn from_snapshot(dir: &Path, epoch: Epoch) -> Result<RocksDB, OpenError> {
        let mut opts = Options::default();
        opts.set_prefix_extractor(SliceTransform::create_fixed_prefix(PREFIX_LEN));
        Ok(RocksDB {
            snapshots: vec![epoch],
            dir: dir.to_path_buf(),
            db: OptimisticTransactionDB::open(&opts, dir.join(PathBuf::from(format!("{epoch:?}"))))
                .map_err(OpenError::RocksDB)?,
        })
    }

    pub fn unsafe_transaction(&self) -> rocksdb::Transaction<'_, OptimisticTransactionDB> {
        self.db.transaction()
    }
}

impl Store for RocksDB {
    type Error = rocksdb::Error;

    fn tip(&self) -> Result<Point, Self::Error> {
        Ok(self
            .db
            .get(KEY_TIP)?
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
        issuer: Option<&'_ PoolId>,
        add: Columns<
            impl Iterator<Item = utxo::Add>,
            impl Iterator<Item = pools::Add>,
            impl Iterator<Item = accounts::Add>,
        >,
        remove: Columns<
            impl Iterator<Item = utxo::Remove>,
            impl Iterator<Item = pools::Remove>,
            impl Iterator<Item = accounts::Remove>,
        >,
    ) -> Result<(), Self::Error> {
        let batch = self.db.transaction();

        let tip: Option<Point> = batch.get(KEY_TIP)?.map(|bytes| {
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
                batch.put(KEY_TIP, as_value(point))?;

                if let Some(issuer) = issuer {
                    slots::rocksdb::put(
                        &batch,
                        &point.slot_or_default(),
                        slots::Row::new(*issuer),
                    )?;
                }

                utxo::rocksdb::add(&batch, add.utxo)?;
                pools::rocksdb::add(&batch, add.pools)?;
                accounts::rocksdb::add(&batch, add.accounts)?;
                utxo::rocksdb::remove(&batch, remove.utxo)?;
                pools::rocksdb::remove(&batch, remove.pools)?;
                accounts::rocksdb::remove(&batch, remove.accounts)?;
            }
        }

        batch.commit()
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
    ) -> Result<(), Self::Error> {
        let snapshot = self.snapshots.last().map(|s| s + 1).unwrap_or(epoch);
        if snapshot == epoch {
            let path = self.dir.join(snapshot.to_string());

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

            checkpoint::Checkpoint::new(&self.db)?.create_checkpoint(path)?;

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

    fn rewards_summary(&self, epoch: Epoch) -> Result<RewardsSummary, Self::Error> {
        let stake_distr = StakeDistributionSnapshot::new(
            &Self::from_snapshot(&self.dir, epoch - 2).unwrap_or_else(|e| {
                panic!(
                    "unable to open database snapshot for epoch {:?}: {:?}",
                    epoch - 2,
                    e
                )
            }),
        )?;
        RewardsSummary::new(
            &Self::from_snapshot(&self.dir, epoch).unwrap_or_else(|e| {
                panic!(
                    "unable to open database snapshot for epoch {:?}: {:?}",
                    epoch, e
                )
            }),
            stake_distr,
        )
    }

    fn with_pots<A>(
        &self,
        mut with: impl FnMut(Box<dyn std::borrow::BorrowMut<pots::Row> + '_>) -> A,
    ) -> Result<A, Self::Error> {
        let db = self.db.transaction();

        let mut err = None;
        let proxy = Box::new(BorrowableProxy::new(pots::rocksdb::get(&db)?, |pots| {
            let put = pots::rocksdb::put(&db, pots).and_then(|_| db.commit());
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

    fn pool(&self, pool: &PoolId) -> Result<Option<pools::Row>, Self::Error> {
        pools::rocksdb::get(&self.db, pool)
    }

    fn resolve_input(
        &self,
        input: &TransactionInput,
    ) -> Result<Option<TransactionOutput>, Self::Error> {
        utxo::rocksdb::get(&self.db, input)
    }

    fn with_utxo(&self, with: impl FnMut(utxo::Iter<'_, '_>)) -> Result<(), rocksdb::Error> {
        with_prefix_iterator(self.db.transaction(), utxo::rocksdb::PREFIX, with)
    }

    fn with_pools(&self, with: impl FnMut(pools::Iter<'_, '_>)) -> Result<(), rocksdb::Error> {
        with_prefix_iterator(self.db.transaction(), pools::rocksdb::PREFIX, with)
    }

    fn with_accounts(
        &self,
        with: impl FnMut(accounts::Iter<'_, '_>),
    ) -> Result<(), rocksdb::Error> {
        with_prefix_iterator(self.db.transaction(), accounts::rocksdb::PREFIX, with)
    }

    fn with_block_issuers(
        &self,
        with: impl FnMut(slots::Iter<'_, '_>),
    ) -> Result<(), rocksdb::Error> {
        with_prefix_iterator(self.db.transaction(), slots::rocksdb::PREFIX, with)
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
) -> Result<(), rocksdb::Error> {
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
        }?;
    }

    db.commit()
}
