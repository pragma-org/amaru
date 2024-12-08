pub(crate) mod common;

use crate::{
    iter::{borrow as iter_borrow, borrow::IterBorrow},
    ledger::{
        kernel::{Epoch, Point, PoolId},
        store::{columns::*, Columns, Store},
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
use tracing::{debug, info, warn};

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
/// * 'utxo:'TransactionInput * TransactionOutput                              *
/// * 'pool:'PoolId           * (PoolParams, Vec<(Option<PoolParams>, Epoch)>) *
/// * 'acct:'StakeCredential  * (Option<PoolId>, Lovelace, Lovelace)           *
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
                debug!(epoch, "found existing ledger snapshot");
                snapshots.push(epoch);
            } else if entry.file_name() != DIR_LIVE_DB {
                warn!(
                    dir_entry = entry.file_name().to_str().unwrap_or_default(),
                    "unexpected file within the database directory folder; ignoring"
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
                info!("point already known; save skipped");
            }
            _ => {
                batch.put(KEY_TIP, as_value(point))?;
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

    fn next_snapshot(&'_ mut self, epoch: Epoch) -> Result<(), Self::Error> {
        let snapshot = self.snapshots.last().map(|s| s + 1).unwrap_or(epoch);
        if snapshot == epoch {
            info!(?epoch, "next snapshot");
            let path = self.dir.join(snapshot.to_string());
            checkpoint::Checkpoint::new(&self.db)?.create_checkpoint(path)?;
            self.snapshots.push(snapshot);
        } else {
            debug!(epoch, "snapshot already taken; ignoring");
        }
        Ok(())
    }

    fn pool(&self, pool: &PoolId) -> Result<Option<pools::Row>, Self::Error> {
        pools::rocksdb::get(&self.db, pool)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::kernel::{encode_bech32, PoolParams, StakeCredential};
    use serde::ser::SerializeStruct;
    use std::collections::BTreeMap;

    struct Snapshot {
        keys: BTreeMap<String, String>,
        scripts: BTreeMap<String, String>,
        pools: BTreeMap<String, PoolParams>,
    }

    impl serde::Serialize for Snapshot {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            let mut s = serializer.serialize_struct("Snapshot", 3)?;
            s.serialize_field("keys", &self.keys)?;
            s.serialize_field("scripts", &self.scripts)?;
            s.serialize_field("stakePoolParameters", &self.pools)?;
            s.end()
        }
    }

    fn new_preprod_snapshot(epoch: Epoch) -> Snapshot {
        let db = RocksDB::from_snapshot(&PathBuf::from("../ledger.db"), epoch).unwrap();

        let mut pools = BTreeMap::new();
        db.with_pools(|rows| {
            for (_, row) in rows {
                if let Some(pool) = row.borrow() {
                    pools.insert(
                        encode_bech32("pool", &pool.current_params.id[..]).unwrap(),
                        pool.current_params.clone(),
                    );
                }
            }
        })
        .unwrap();

        let mut accounts_scripts = BTreeMap::new();
        let mut accounts_keys = BTreeMap::new();
        db.with_accounts(|rows| {
            for (key, row) in rows {
                if let Some(account) = row.borrow() {
                    // NOTE: Snapshots from the Haskell node actually excludes:
                    //
                    // 1. Any stake credential that is registered but not delegated.
                    // 2. Any stake credential delegated to a now-retired stake pool.
                    if let Some(pool) = account.delegatee {
                        let pool_str = encode_bech32("pool", &pool[..]).unwrap();
                        if pools.contains_key(&pool_str) {
                            match key {
                                StakeCredential::ScriptHash(script) => {
                                    accounts_scripts.insert(hex::encode(script), pool_str);
                                }
                                StakeCredential::AddrKeyhash(key) => {
                                    accounts_keys.insert(hex::encode(key), pool_str);
                                }
                            }
                        }
                    }
                }
            }
        })
        .unwrap();

        Snapshot {
            keys: accounts_keys,
            scripts: accounts_scripts,
            pools,
        }
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_163() {
        let snapshot = new_preprod_snapshot(163);
        insta::assert_json_snapshot!(snapshot);
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_164() {
        let snapshot = new_preprod_snapshot(164);
        insta::assert_json_snapshot!(snapshot);
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_165() {
        let snapshot = new_preprod_snapshot(165);
        insta::assert_json_snapshot!(snapshot);
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_166() {
        let snapshot = new_preprod_snapshot(166);
        insta::assert_json_snapshot!(snapshot);
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_167() {
        let snapshot = new_preprod_snapshot(167);
        insta::assert_json_snapshot!(snapshot);
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_168() {
        let snapshot = new_preprod_snapshot(168);
        insta::assert_json_snapshot!(snapshot);
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_169() {
        let snapshot = new_preprod_snapshot(169);
        insta::assert_json_snapshot!(snapshot);
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_170() {
        let snapshot = new_preprod_snapshot(170);
        insta::assert_json_snapshot!(snapshot);
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_171() {
        let snapshot = new_preprod_snapshot(171);
        insta::assert_json_snapshot!(snapshot);
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_172() {
        let snapshot = new_preprod_snapshot(172);
        insta::assert_json_snapshot!(snapshot);
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_173() {
        let snapshot = new_preprod_snapshot(173);
        insta::assert_json_snapshot!(snapshot);
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_174() {
        let snapshot = new_preprod_snapshot(174);
        insta::assert_json_snapshot!(snapshot);
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_175() {
        let snapshot = new_preprod_snapshot(175);
        insta::assert_json_snapshot!(snapshot);
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_176() {
        let snapshot = new_preprod_snapshot(176);
        insta::assert_json_snapshot!(snapshot);
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_177() {
        let snapshot = new_preprod_snapshot(177);
        insta::assert_json_snapshot!(snapshot);
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_178() {
        let snapshot = new_preprod_snapshot(178);
        insta::assert_json_snapshot!(snapshot);
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_179() {
        let snapshot = new_preprod_snapshot(179);
        insta::assert_json_snapshot!(snapshot);
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_180() {
        let snapshot = new_preprod_snapshot(180);
        insta::assert_json_snapshot!(snapshot);
    }
}
