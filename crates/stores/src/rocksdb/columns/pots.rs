use super::super::common::{as_value, PREFIX_LEN};
use crate::rocksdb::scolumns::pots::Row;
use rocksdb::{self, Transaction};

/// Name prefixed used for storing protocol pots. UTF-8 encoding for "pots"
pub const PREFIX: [u8; PREFIX_LEN] = [0x70, 0x6f, 0x74, 0x73];

pub fn get<DB>(db: &Transaction<'_, DB>) -> Result<Row, rocksdb::Error> {
    Ok(Row::unsafe_decode(db.get(PREFIX)?.unwrap_or_else(|| {
        panic!("no protocol pots (treasury, reserves, fees, ...) found")
    })))
}

pub fn put<DB>(db: &Transaction<'_, DB>, row: Row) -> Result<(), rocksdb::Error> {
    db.put(PREFIX, as_value(row))
}
