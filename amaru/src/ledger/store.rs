use super::kernel::{Epoch, Point, PoolId, PoolParams, TransactionInput, TransactionOutput};
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

    fn get_tip(&self) -> Result<Point, Self::Error>;

    fn get_pool(&self, pool: &PoolId, epoch: Epoch) -> Result<Option<PoolParams>, Self::Error>;
}

pub struct Add<'a> {
    pub utxo: Box<dyn Iterator<Item = (TransactionInput, TransactionOutput)> + 'a>,
    pub pools: Box<dyn Iterator<Item = (PoolId, PoolParams, Epoch)> + 'a>,
}

impl<'a> Default for Add<'a> {
    fn default() -> Self {
        Self {
            utxo: Box::new(iter::empty())
                as Box<dyn Iterator<Item = (TransactionInput, TransactionOutput)> + 'a>,
            pools: Box::new(iter::empty())
                as Box<dyn Iterator<Item = (PoolId, PoolParams, Epoch)> + 'a>,
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

// rocksDB implementation
// ----------------------------------------------------------------------------

pub mod impl_rocksdb {
    use super::*;
    use pallas_codec::minicbor::{self as cbor, Encode};
    use rocksdb;

    const SEPARATOR: [u8; 1] = [0x3a];
    const PREFIX_UTXO: [u8; 4] = [0x75, 0x74, 0x78, 0x6f];
    const PREFIX_POOL: [u8; 4] = [0x70, 0x6f, 0x6f, 0x6c];
    const KEY_TIP: &str = "tip";

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
        db: rocksdb::OptimisticTransactionDB,
    }

    impl RocksDB {
        pub fn new(target: &Path) -> Result<RocksDB, rocksdb::Error> {
            Ok(RocksDB {
                db: rocksdb::OptimisticTransactionDB::open_default(target)?,
            })
        }
    }

    impl Store for RocksDB {
        type Error = rocksdb::Error;

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

                    for (pool, params, epoch) in add.pools {
                        // Pool parameters are stored in an epoch-aware fashion.
                        //
                        // - If no parameters exist for the pool, we can immediately create a new
                        //   entry.
                        //
                        // - If one already exists, then the parameters are stashed until the next
                        //   epoch boundary.
                        //
                        let params = match batch.get(as_key(&PREFIX_POOL, pool))? {
                            None => as_value(EpochAwarePoolParams::new(params)),
                            Some(existing_params) => {
                                EpochAwarePoolParams::extend(existing_params, (Some(params), epoch))
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
                                EpochAwarePoolParams::extend(existing_params, (None, epoch)),
                            )?,
                        };
                    }
                }
            }

            batch.commit()
        }

        fn get_pool(
            &self,
            _pool: &PoolId,
            _epoch: Epoch,
        ) -> Result<Option<PoolParams>, Self::Error> {
            Ok(None)
        }

        fn get_tip(&self) -> Result<Point, Self::Error> {
            todo!()
        }
    }

    /// Serialize some value to be used as key, with the given prefix. We use common prefixes for
    /// objects of the same type to emulate some kind of table organization within RocksDB. This
    /// allows to iterate over the store by specific prefixes and also avoid key-clashes across
    /// objects that could otherwise have an identical key.
    fn as_key<T: Encode<()> + std::fmt::Debug>(prefix: &[u8], key: T) -> Vec<u8> {
        as_bytes(&[prefix, &SEPARATOR[..]].concat(), key)
    }

    /// A simple helper function to encode any (serialisable) value to CBOR bytes.
    fn as_value<T: Encode<()> + std::fmt::Debug>(value: T) -> Vec<u8> {
        as_bytes(&[], value)
    }

    /// A simple helper function to encode any (serialisable) value to CBOR bytes.
    fn as_bytes<T: Encode<()> + std::fmt::Debug>(prefix: &[u8], value: T) -> Vec<u8> {
        let mut buffer = Vec::from(prefix);
        cbor::encode(value, &mut buffer)
            .unwrap_or_else(|e| panic!("unable to encode value to CBOR: {e:?}"));
        buffer
    }

    // EpochAwarePoolParams
    // ----------------------------------------------------------------------------

    #[derive(Debug)]
    struct EpochAwarePoolParams {
        current_params: PoolParams,
        future_params: Vec<(PoolParams, Epoch)>,
    }

    impl EpochAwarePoolParams {
        pub fn new(current_params: PoolParams) -> Self {
            Self {
                current_params,
                future_params: Vec::new(),
            }
        }

        pub fn extend(mut bytes: Vec<u8>, future_params: (Option<PoolParams>, Epoch)) -> Vec<u8> {
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

    impl<C> cbor::encode::Encode<C> for EpochAwarePoolParams {
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
}
