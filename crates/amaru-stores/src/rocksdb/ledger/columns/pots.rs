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
use amaru_ledger::store::{
    columns::{pots::Row, unsafe_decode},
    StoreError,
};
use rocksdb::Transaction;

/// Name prefixed used for storing protocol pots. UTF-8 encoding for "pots"
pub const PREFIX: [u8; PREFIX_LEN] = [0x70, 0x6f, 0x74, 0x73];

pub fn get(
    db_get: impl Fn(&[u8]) -> Result<Option<Vec<u8>>, rocksdb::Error>,
) -> Result<Row, StoreError> {
    let bytes = db_get(&PREFIX);
    Ok(bytes
        .map_err(|err| StoreError::Internal(err.into()))?
        .map(unsafe_decode::<Row>)
        .unwrap_or_default())
}

pub fn put<DB>(db: &Transaction<'_, DB>, row: Row) -> Result<(), StoreError> {
    db.put(PREFIX, as_value(row))
        .map_err(|err| StoreError::Internal(err.into()))
}
