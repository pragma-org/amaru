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

use super::super::common::{PREFIX_LEN, as_value};
use crate::rocksdb::scolumns::pots::Row;
use amaru_ledger::store::StoreError;
use rocksdb::Transaction;

/// Name prefixed used for storing protocol pots. UTF-8 encoding for "pots"
pub const PREFIX: [u8; PREFIX_LEN] = [0x70, 0x6f, 0x74, 0x73];

#[allow(clippy::panic)]
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
