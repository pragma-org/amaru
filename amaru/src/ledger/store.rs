use super::kernel::{TransactionInput, TransactionOutput};
use std::path::Path;

// Store
// ----------------------------------------------------------------------------

pub trait Store {
    type Error;

    fn save(
        &mut self,
        add: Box<dyn Iterator<Item = (TransactionInput, TransactionOutput)>>,
        remove: Box<dyn Iterator<Item = TransactionInput>>,
    ) -> Result<(), Self::Error>;
}

// rocksDB implementation
// ----------------------------------------------------------------------------

pub mod impl_rocksdb {
    use super::*;
    use pallas_codec::minicbor::{self as cbor, Encode};
    use rocksdb;

    pub(crate) struct RocksDB {
        db: rocksdb::DB,
    }

    impl RocksDB {
        pub fn new(target: &Path) -> Result<RocksDB, rocksdb::Error> {
            Ok(RocksDB {
                db: rocksdb::DB::open_default(target)?,
            })
        }
    }

    impl Store for RocksDB {
        type Error = rocksdb::Error;

        fn save(
            &mut self,
            add: Box<dyn Iterator<Item = (TransactionInput, TransactionOutput)>>,
            remove: Box<dyn Iterator<Item = TransactionInput>>,
        ) -> Result<(), Self::Error> {
            let mut batch = rocksdb::WriteBatchWithTransaction::<false>::default();

            for (input, output) in add {
                batch.put(as_bytes(input), as_bytes(output));
            }

            for input in remove {
                batch.delete(as_bytes(input));
            }

            self.db.write(batch)
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
