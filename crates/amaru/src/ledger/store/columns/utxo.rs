use crate::{
    iter::borrow as iter_borrow,
    ledger::kernel::{TransactionInput, TransactionOutput},
};

pub type Key = TransactionInput;

pub type Value = TransactionOutput;

/// Iterator used to browse rows from the Pools column. Meant to be referenced using qualified imports.
pub type Iter<'a, 'b> = iter_borrow::IterBorrow<'a, 'b, Key, Option<Value>>;

pub mod rocksdb {
    use crate::ledger::{
        kernel::{TransactionInput, TransactionOutput},
        store::rocksdb::common::{as_key, as_value, PREFIX_LEN},
    };
    use pallas_codec::minicbor::{self as cbor};
    use rocksdb::{self, OptimisticTransactionDB, ThreadMode, Transaction};

    /// Name prefixed used for storing UTxO entries. UTF-8 encoding for "utxo"
    pub const PREFIX: [u8; PREFIX_LEN] = [0x75, 0x74, 0x78, 0x6f];

    pub fn get<T: ThreadMode>(
        db: &OptimisticTransactionDB<T>,
        input: &TransactionInput,
    ) -> Result<Option<TransactionOutput>, rocksdb::Error> {
        Ok(db.get(as_key(&PREFIX, input))?.map(|bytes| {
            cbor::decode(&bytes).unwrap_or_else(|e| {
                panic!(
                    "unable to decode TransactionOutput from CBOR ({}): {e:?}",
                    hex::encode(&bytes)
                )
            })
        }))
    }

    pub fn add<DB>(
        db: &Transaction<'_, DB>,
        rows: impl Iterator<Item = (super::Key, super::Value)>,
    ) -> Result<(), rocksdb::Error> {
        for (input, output) in rows {
            db.put(as_key(&PREFIX, input), as_value(output))?;
        }

        Ok(())
    }

    pub fn remove<DB>(
        db: &Transaction<'_, DB>,
        rows: impl Iterator<Item = super::Key>,
    ) -> Result<(), rocksdb::Error> {
        for input in rows {
            db.delete(as_key(&PREFIX, input))?;
        }

        Ok(())
    }
}
