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
                info!(epoch, "found existing ledger snapshot");
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

        Ok(RocksDB {
            snapshots,
            dir: dir.to_path_buf(),
            db: OptimisticTransactionDB::open(&opts, dir.join("live"))
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
            (Point::Specific(new, _), Some(Point::Specific(current, _))) if *new <= current => {}
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

    fn most_recent_snapshot(&'_ self) -> Option<Epoch> {
        self.snapshots.last().cloned()
    }

    fn next_snapshot(&'_ mut self, epoch: Epoch) -> Result<(), Self::Error> {
        let snapshot = self.most_recent_snapshot().map(|n| n + 1).unwrap_or(epoch);
        if snapshot == epoch {
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

    fn with_pools(&self, with: impl Fn(pools::Iter<'_, '_>)) -> Result<(), rocksdb::Error> {
        let db = self.db.transaction();

        let mut iterator =
            iter_borrow::new(db.prefix_iterator(pools::rocksdb::PREFIX).map(|item| {
                // TODO: clarify what kind of errors can come from the database at this point.
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
