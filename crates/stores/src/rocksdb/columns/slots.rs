use super::super::common::{as_key, as_value, PREFIX_LEN};
use crate::rocksdb::scolumns::slots::Row;
use amaru_ledger::{kernel::Slot, store::StoreError};
use rocksdb::{OptimisticTransactionDB, ThreadMode, Transaction};

/// Name prefixed used for storing Pool entries. UTF-8 encoding for "slot"
pub const PREFIX: [u8; PREFIX_LEN] = [0x73, 0x6c, 0x6f, 0x74];

pub fn get<T: ThreadMode>(
    db: &OptimisticTransactionDB<T>,
    slot: &Slot,
) -> Result<Option<Row>, StoreError> {
    Ok(db
        .get(as_key(&PREFIX, slot))
        .map_err(|err| StoreError::Internal(err.into()))?
        .map(Row::unsafe_decode))
}

pub fn put<DB>(db: &Transaction<'_, DB>, slot: &Slot, row: Row) -> Result<(), StoreError> {
    db.put(as_key(&PREFIX, slot), as_value(row))
        .map_err(|err| StoreError::Internal(err.into()))
}
