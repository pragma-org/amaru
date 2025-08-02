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

use crate::rocksdb::common::{as_key, as_value, PREFIX_LEN};
use amaru_ledger::store::{
    columns::{
        slots::{Key, Value},
        unsafe_decode,
    },
    StoreError,
};
use rocksdb::{OptimisticTransactionDB, ThreadMode, Transaction};

/// Name prefixed used for storing Pool entries. UTF-8 encoding for "slot"
pub const PREFIX: [u8; PREFIX_LEN] = [0x73, 0x6c, 0x6f, 0x74];

pub fn get<T: ThreadMode>(
    db: &OptimisticTransactionDB<T>,
    key: &Key,
) -> Result<Option<Value>, StoreError> {
    Ok(db
        .get(as_key(&PREFIX, key))
        .map_err(|err| StoreError::Internal(err.into()))?
        .map(unsafe_decode::<Value>))
}

pub fn put<DB>(db: &Transaction<'_, DB>, key: &Key, value: Value) -> Result<(), StoreError> {
    db.put(as_key(&PREFIX, key), as_value(value))
        .map_err(|err| StoreError::Internal(err.into()))
}
