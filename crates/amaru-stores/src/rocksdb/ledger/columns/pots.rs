// Copyright 2024 PRAGMA
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::rocksdb::common::{as_value, PREFIX_LEN};
use amaru_ledger::store::{columns::pots::Row, StoreError};
use rocksdb::{Transaction, DB};

/// Name prefixed used for storing protocol pots. UTF-8 encoding for "pots"
pub const PREFIX: [u8; PREFIX_LEN] = [0x70, 0x6f, 0x74, 0x73];

fn get_row_from_bytes(bytes: Result<Option<Vec<u8>>, rocksdb::Error>) -> Result<Row, StoreError> {
    Ok(bytes
        .map_err(|err| StoreError::Internal(err.into()))?
        .map(Row::unsafe_decode)
        .unwrap_or_default())
}

pub fn get<DB>(db: &Transaction<'_, DB>) -> Result<Row, StoreError> {
    let bytes = db.get(PREFIX);
    get_row_from_bytes(bytes)
}

pub fn get_from_db(db: &DB) -> Result<Row, StoreError> {
    let bytes = db.get(PREFIX);
    get_row_from_bytes(bytes)
}

pub fn put<DB>(db: &Transaction<'_, DB>, row: Row) -> Result<(), StoreError> {
    db.put(PREFIX, as_value(row))
        .map_err(|err| StoreError::Internal(err.into()))
}
