use crate::{
    iter::borrow as iter_borrow,
    ledger::kernel::{TransactionInput, TransactionOutput},
};

pub type Add = (TransactionInput, TransactionOutput);

pub type Remove = TransactionInput;

/// Iterator used to browse rows from the Pools column. Meant to be referenced using qualified imports.
pub type Iter<'a, 'b> =
    iter_borrow::IterBorrow<'a, 'b, TransactionInput, Option<TransactionOutput>>;

pub mod rocksdb {
    use crate::ledger::store::rocksdb::common::{as_key, as_value, PREFIX_LEN};
    use rocksdb::{self, Transaction};

    /// Name prefixed used for storing UTxO entries. UTF-8 encoding for "utxo"
    pub const PREFIX: [u8; PREFIX_LEN] = [0x75, 0x74, 0x78, 0x6f];

    pub fn add<DB>(
        db: &Transaction<'_, DB>,
        rows: impl Iterator<Item = super::Add>,
    ) -> Result<(), rocksdb::Error> {
        for (input, output) in rows {
            db.put(as_key(&PREFIX, input), as_value(output))?;
        }

        Ok(())
    }

    pub fn remove<DB>(
        db: &Transaction<'_, DB>,
        rows: impl Iterator<Item = super::Remove>,
    ) -> Result<(), rocksdb::Error> {
        for input in rows {
            db.delete(as_key(&PREFIX, input))?;
        }

        Ok(())
    }
}
