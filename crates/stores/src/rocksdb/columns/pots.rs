use super::super::common::{as_value, PREFIX_LEN};
use crate::rocksdb::scolumns::pots::Row;
use amaru_ledger::store::StoreError;
use rocksdb::Transaction;

/// Name prefixed used for storing protocol pots. UTF-8 encoding for "pots"
pub const PREFIX: [u8; PREFIX_LEN] = [0x70, 0x6f, 0x74, 0x73];

pub fn get<DB>(db: &Transaction<'_, DB>) -> Result<Row, StoreError> {
    Ok(Row::unsafe_decode(
        db.get(PREFIX)
            .map_err(|err| StoreError::Internal(err.into()))?
            .unwrap_or_else(|| panic!("no protocol pots (treasury, reserves, fees, ...) found")),
    ))
}

pub fn put<DB>(db: &Transaction<'_, DB>, row: Row) -> Result<(), StoreError> {
    db.put(PREFIX, as_value(row))
        .map_err(|err| StoreError::Internal(err.into()))
}
