pub(crate) mod common;

use crate::{
    iter::borrow as iter_borrow,
    ledger::{
        kernel::{Epoch, Point, PoolId, PoolParams, TransactionInput, TransactionOutput},
        store::{
            columns::{pools, utxo},
            Columns, Store,
        },
    },
};
use ::rocksdb::{self, checkpoint, OptimisticTransactionDB, Options, SliceTransform};
use common::{as_value, PREFIX_LEN};
use miette::Diagnostic;
use pallas_codec::minicbor::{self as cbor};
use std::{
    fs, io,
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
            impl Iterator<Item = (TransactionInput, TransactionOutput)>,
            impl Iterator<Item = (PoolParams, Epoch)>,
        >,
        remove: Columns<
            impl Iterator<Item = TransactionInput>,
            impl Iterator<Item = (PoolId, Epoch)>,
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
                utxo::rocksdb::remove(&batch, remove.utxo)?;
                pools::rocksdb::remove(&batch, remove.pools)?;
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

    fn with_pools(&self, mut with: impl FnMut(pools::Iter<'_, '_>)) -> Result<(), rocksdb::Error> {
        let db = self.db.transaction();

        let mut iterator =
            iter_borrow::new(db.prefix_iterator(pools::rocksdb::PREFIX).map(|item| {
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

        db.commit()?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::kernel::encode_bech32;
    use std::collections::BTreeMap;

    fn compare_preprod_snapshot(epoch: Epoch) -> BTreeMap<String, PoolParams> {
        let mut pools = BTreeMap::new();

        let db = RocksDB::from_snapshot(&PathBuf::from("../ledger.db"), epoch).unwrap();

        db.with_pools(|iterator| {
            for row in iterator {
                if let Some(pool) = row.borrow() {
                    pools.insert(
                        encode_bech32("pool", &pool.current_params.id[..]).unwrap(),
                        pool.current_params.clone(),
                    );
                }
            }
        })
        .unwrap();

        pools
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_163() {
        let pools = compare_preprod_snapshot(163);
        insta::assert_json_snapshot!(pools);
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_164() {
        let pools = compare_preprod_snapshot(164);
        insta::assert_json_snapshot!(pools);
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_165() {
        let pools = compare_preprod_snapshot(165);
        insta::assert_json_snapshot!(pools);
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_166() {
        let pools = compare_preprod_snapshot(166);
        insta::assert_json_snapshot!(pools);
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_167() {
        let pools = compare_preprod_snapshot(167);
        insta::assert_json_snapshot!(pools);
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_168() {
        let pools = compare_preprod_snapshot(168);
        insta::assert_json_snapshot!(pools);
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_169() {
        let pools = compare_preprod_snapshot(169);
        insta::assert_json_snapshot!(pools);
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_170() {
        let pools = compare_preprod_snapshot(170);
        insta::assert_json_snapshot!(pools);
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_171() {
        let pools = compare_preprod_snapshot(171);
        insta::assert_json_snapshot!(pools);
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_172() {
        let pools = compare_preprod_snapshot(172);
        insta::assert_json_snapshot!(pools);
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_173() {
        let pools = compare_preprod_snapshot(173);
        insta::assert_json_snapshot!(pools);
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_174() {
        let pools = compare_preprod_snapshot(174);
        insta::assert_json_snapshot!(pools);
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_175() {
        let pools = compare_preprod_snapshot(175);
        insta::assert_json_snapshot!(pools);
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_176() {
        let pools = compare_preprod_snapshot(176);
        insta::assert_json_snapshot!(pools);
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_177() {
        let pools = compare_preprod_snapshot(177);
        insta::assert_json_snapshot!(pools);
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_178() {
        let pools = compare_preprod_snapshot(178);
        insta::assert_json_snapshot!(pools);
    }

    #[test]
    #[ignore]
    fn compare_preprod_snapshot_179() {
        let pools = compare_preprod_snapshot(179);
        insta::assert_json_snapshot!(pools);
    }
}
