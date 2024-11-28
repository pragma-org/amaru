use super::kernel::{Epoch, Point, PoolId, PoolParams, TransactionInput, TransactionOutput};
use pallas_codec::minicbor::{self as cbor};
use std::{iter, path::Path};

// Store
// ----------------------------------------------------------------------------

pub trait Store {
    type Error;

    fn save<'a>(
        &'_ self,
        point: &'_ Point,
        add: Add<'a>,
        remove: Remove<'a>,
    ) -> Result<(), Self::Error>;

    fn most_recent_snapshot(&self) -> Option<Epoch>;

    /// Construct and save on-disk a snapshot of the store. The epoch number is used when
    /// there's no existing snapshot and, to ensure that snapshots are taken in order.
    fn next_snapshot(&mut self, epoch: Epoch) -> Result<(), Self::Error>;

    fn get_tip(&self) -> Result<Point, Self::Error>;

    fn get_pool(&self, pool: &PoolId) -> Result<Option<PoolParamsUpdates>, Self::Error>;
}

pub struct Add<'a> {
    pub utxo: Box<dyn Iterator<Item = (TransactionInput, TransactionOutput)> + 'a>,
    pub pools: Box<dyn Iterator<Item = (PoolParams, Epoch)> + 'a>,
}

impl<'a> Default for Add<'a> {
    fn default() -> Self {
        Self {
            utxo: Box::new(iter::empty())
                as Box<dyn Iterator<Item = (TransactionInput, TransactionOutput)> + 'a>,
            pools: Box::new(iter::empty()) as Box<dyn Iterator<Item = (PoolParams, Epoch)> + 'a>,
        }
    }
}

pub struct Remove<'a> {
    pub utxo: Box<dyn Iterator<Item = TransactionInput> + 'a>,
    pub pools: Box<dyn Iterator<Item = (PoolId, Epoch)> + 'a>,
}

impl<'a> Default for Remove<'a> {
    fn default() -> Self {
        Self {
            utxo: Box::new(iter::empty()) as Box<dyn Iterator<Item = TransactionInput> + 'a>,
            pools: Box::new(iter::empty()) as Box<dyn Iterator<Item = (PoolId, Epoch)> + 'a>,
        }
    }
}

// PoolParamsUpdates
// ----------------------------------------------------------------------------

#[derive(Debug)]
pub struct PoolParamsUpdates {
    pub current_params: PoolParams,
    pub future_params: Vec<(PoolParams, Epoch)>,
}

impl PoolParamsUpdates {
    fn new(current_params: PoolParams) -> Self {
        Self {
            current_params,
            future_params: Vec::new(),
        }
    }

    fn extend(mut bytes: Vec<u8>, future_params: (Option<PoolParams>, Epoch)) -> Vec<u8> {
        let tail = bytes.split_off(bytes.len() - 2);
        assert_eq!(
            tail,
            vec![0xFF, 0xFF],
            "invalid tail of serialized pool parameters"
        );
        cbor::encode(future_params, &mut bytes)
            .unwrap_or_else(|e| panic!("unable to encode value to CBOR: {e:?}"));
        [bytes, tail].concat()
    }
}

impl<C> cbor::encode::Encode<C> for PoolParamsUpdates {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        // NOTE: We explicitly enforce the use of *indefinite* arrays here because it allows us
        // to extend the serialized data easily without having to deserialise it.
        e.begin_array()?;
        e.encode_with(&self.current_params, ctx)?;
        e.begin_array()?;
        for update in self.future_params.iter() {
            e.encode_with(update, ctx)?;
        }
        e.end()?;
        e.end()?;
        Ok(())
    }
}

impl<'a, C> cbor::decode::Decode<'a, C> for PoolParamsUpdates {
    fn decode(d: &mut cbor::Decoder<'a>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        d.array()?;
        let current_params = d.decode_with(ctx)?;

        let mut iter = d.array_iter()?;

        let mut future_params = Vec::new();
        for item in &mut iter {
            future_params.push(item?);
        }

        Ok(PoolParamsUpdates {
            current_params,
            future_params,
        })
    }
}

// rocksDB implementation
// ----------------------------------------------------------------------------

pub mod impl_rocksdb {
    use super::*;
    use miette::Diagnostic;
    use pallas_codec::minicbor::{self as cbor};
    use rocksdb;
    use std::{fs, io, path::PathBuf};
    use tracing::{debug, info, warn};

    /// Name separator used between keys prefix and their actual payload. UTF-8 encoding for ':'
    const SEPARATOR: [u8; 1] = [0x3a];

    /// Name prefixed used for storing UTxO entries. UTF-8 encoding for "utxo"
    const PREFIX_UTXO: [u8; 4] = [0x75, 0x74, 0x78, 0x6f];

    /// Name prefixed used for storing pool entries. UTF-8 encoding for "pool"
    const PREFIX_POOL: [u8; 4] = [0x70, 0x6f, 0x6f, 0x6c];

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
        db: rocksdb::OptimisticTransactionDB,

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

            Ok(RocksDB {
                snapshots,
                dir: dir.to_path_buf(),
                db: rocksdb::OptimisticTransactionDB::open_default(dir.join("live"))
                    .map_err(OpenError::RocksDB)?,
            })
        }
    }

    impl Store for RocksDB {
        type Error = rocksdb::Error;

        fn most_recent_snapshot(&'_ self) -> Option<Epoch> {
            self.snapshots.last().cloned()
        }

        fn next_snapshot(&'_ mut self, epoch: Epoch) -> Result<(), Self::Error> {
            let snapshot = self.most_recent_snapshot().map(|n| n + 1).unwrap_or(epoch);
            if snapshot == epoch {
                let path = self.dir.join(snapshot.to_string());
                rocksdb::checkpoint::Checkpoint::new(&self.db)?.create_checkpoint(path)?;
                self.snapshots.push(snapshot);
            } else {
                debug!(epoch, "snapshot already taken; ignoring");
            }
            Ok(())
        }

        fn save<'a>(
            &'_ self,
            point: &'_ Point,
            add: Add<'a>,
            remove: Remove<'a>,
        ) -> Result<(), Self::Error> {
            let batch = self.db.transaction();

            let tip: Option<Point> = batch
                .get(KEY_TIP)?
                .map(|bytes| cbor::decode(&bytes))
                .transpose()
                .expect("unable to decode database tip");

            match (point, tip) {
                (Point::Specific(new, _), Some(Point::Specific(current, _))) if *new <= current => {
                }
                _ => {
                    // TODO: This code is going to grow out of control rapidly. We should move the
                    // add/remove logic and associated (de)serialization logic under specific
                    // types and keep this level relatively high.

                    batch.put(KEY_TIP, as_value(point))?;

                    for (input, output) in add.utxo {
                        batch.put(as_key(&PREFIX_UTXO, input), as_value(output))?;
                    }

                    for (params, epoch) in add.pools {
                        let pool = params.id;

                        // Pool parameters are stored in an epoch-aware fashion.
                        //
                        // - If no parameters exist for the pool, we can immediately create a new
                        //   entry.
                        //
                        // - If one already exists, then the parameters are stashed until the next
                        //   epoch boundary.
                        //
                        // TODO: We might want to define a MERGE OPERATOR to speed this up if
                        // necessary.
                        let params = match batch.get(as_key(&PREFIX_POOL, pool))? {
                            None => as_value(PoolParamsUpdates::new(params)),
                            Some(existing_params) => {
                                PoolParamsUpdates::extend(existing_params, (Some(params), epoch))
                            }
                        };

                        batch.put(as_key(&PREFIX_POOL, pool), params)?;
                    }

                    for input in remove.utxo {
                        batch.delete(as_key(&PREFIX_UTXO, input))?;
                    }

                    for (pool, epoch) in remove.pools {
                        // Similarly, we do not delete pool immediately but rather schedule the
                        // removal as an empty parameter update. The 'pool reaping' happens on
                        // every epoch boundary in a separate thread.
                        match batch.get(as_key(&PREFIX_POOL, pool))? {
                            None => (),
                            Some(existing_params) => batch.put(
                                as_key(&PREFIX_POOL, pool),
                                PoolParamsUpdates::extend(existing_params, (None, epoch)),
                            )?,
                        };
                    }
                }
            }

            batch.commit()
        }

        fn get_pool(&self, pool: &PoolId) -> Result<Option<PoolParamsUpdates>, Self::Error> {
            Ok(self
                .db
                .get(as_key(&PREFIX_POOL, pool))?
                .map(|bytes| cbor::decode(&bytes))
                .transpose()
                .expect("unable to decode database's pool"))
        }

        fn get_tip(&self) -> Result<Point, Self::Error> {
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
    }

    /// Serialize some value to be used as key, with the given prefix. We use common prefixes for
    /// objects of the same type to emulate some kind of table organization within RocksDB. This
    /// allows to iterate over the store by specific prefixes and also avoid key-clashes across
    /// objects that could otherwise have an identical key.
    fn as_key<T: cbor::Encode<()> + std::fmt::Debug>(prefix: &[u8], key: T) -> Vec<u8> {
        as_bytes(&[prefix, &SEPARATOR[..]].concat(), key)
    }

    /// A simple helper function to encode any (serialisable) value to CBOR bytes.
    fn as_value<T: cbor::Encode<()> + std::fmt::Debug>(value: T) -> Vec<u8> {
        as_bytes(&[], value)
    }

    /// A simple helper function to encode any (serialisable) value to CBOR bytes.
    fn as_bytes<T: cbor::Encode<()> + std::fmt::Debug>(prefix: &[u8], value: T) -> Vec<u8> {
        let mut buffer = Vec::from(prefix);
        cbor::encode(value, &mut buffer)
            .unwrap_or_else(|e| panic!("unable to encode value to CBOR: {e:?}"));
        buffer
    }
}
