use super::kernel::{Point, TransactionInput, TransactionOutput};
use std::path::Path;

// Store
// ----------------------------------------------------------------------------

pub trait Store {
    type Error;

    fn save<'a>(
        &'_ mut self,
        point: &'_ Point,
        add: Box<dyn Iterator<Item = (TransactionInput, TransactionOutput)> + 'a>,
        remove: Box<dyn Iterator<Item = TransactionInput> + 'a>,
    ) -> Result<(), Self::Error>;
}

// rocksDB implementation
// ----------------------------------------------------------------------------

pub mod impl_rocksdb {
    use super::*;
    use pallas_codec::minicbor::{self as cbor, Encode};
    use rocksdb;

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
            &'_ mut self,
            point: &'_ Point,
            add: Box<dyn Iterator<Item = (TransactionInput, TransactionOutput)> + 'a>,
            remove: Box<dyn Iterator<Item = TransactionInput> + 'a>,
        ) -> Result<(), Self::Error> {
            let batch = self.db.transaction();

            let tip: Option<Point> = batch
                .get("tip")?
                .map(|bytes| cbor::decode(&bytes))
                .transpose()
                .expect("unable to decode database tip");

            match (point, tip) {
                (Point::Specific(new, _), Some(Point::Specific(current, _))) if *new <= current => {
                }
                _ => {
                    batch.put("tip", as_bytes(point))?;

                    for (input, output) in add {
                        batch.put(as_bytes(input), as_bytes(output))?;
                    }

                    for input in remove {
                        batch.delete(as_bytes(input))?;
                    }
                }
            }

            batch.commit()
        }
    }

    /// A simple helper function to encode any (serialisable) value to CBOR bytes.
    fn as_bytes<T: Encode<()> + std::fmt::Debug>(value: T) -> Vec<u8> {
        let mut buffer = Vec::new();
        cbor::encode(value, &mut buffer)
            .unwrap_or_else(|e| panic!("unable to encode value to CBOR: {e:?}"));
        buffer
    }
}
