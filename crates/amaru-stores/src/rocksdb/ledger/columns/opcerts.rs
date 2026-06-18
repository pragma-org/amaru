// Copyright 2026 PRAGMA
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

use amaru_ledger::store::{
    StoreError,
    columns::{
        opcerts::{Key, Value},
        unsafe_decode,
    },
};
use amaru_observability::trace_span;
use rocksdb::{DBPinnableSlice, Transaction};

use crate::rocksdb::common::{PREFIX_LEN, as_key, as_value};

/// Name prefixed used for storing last opcerts sequence numbers entries. UTF-8 encoding for "opce"
pub const PREFIX: [u8; PREFIX_LEN] = *b"opce";

pub fn get<'a>(
    db_get: impl Fn(&[u8]) -> Result<Option<DBPinnableSlice<'a>>, rocksdb::Error>,
    key: &Key,
) -> Result<Option<Value>, StoreError> {
    let _span = trace_span!(
        amaru_observability::amaru::stores::ledger::columns::LAST_OPCERT_SEQUENCE_NUMBER_GET,
        db_system_name = "rocksdb".to_string(),
        db_operation_name = "get".to_string(),
        db_collection_name = "opce".to_string()
    );
    let _guard = _span.enter();

    let key = as_key(&PREFIX, key);
    Ok(db_get(&key).map_err(|err| StoreError::Internal(err.into()))?.map(|d| unsafe_decode::<Value>(&d)))
}

/// This requires a transaction since it is executed in the context of applying a full block to the ledger
pub fn put<DB>(db: &Transaction<'_, DB>, key: &Key, value: Value) -> Result<(), StoreError> {
    let _span = trace_span!(
        amaru_observability::amaru::stores::ledger::columns::LAST_OPCERT_SEQUENCE_NUMBER_PUT,
        db_system_name = "rocksdb".to_string(),
        db_operation_name = "put".to_string(),
        db_collection_name = "opce".to_string()
    );
    let _guard = _span.enter();

    db.put(as_key(&PREFIX, key), as_value(value)).map_err(|err| StoreError::Internal(err.into()))
}

/// This requires a transaction since it is executed in the context of applying a full block to the ledger
pub fn remove<DB>(db: &Transaction<'_, DB>, key: &Key) -> Result<(), StoreError> {
    let _span = trace_span!(
        amaru_observability::amaru::stores::ledger::columns::LAST_OPCERT_SEQUENCE_NUMBER_REMOVE,
        db_system_name = "rocksdb".to_string(),
        db_operation_name = "remove".to_string(),
        db_collection_name = "opce".to_string()
    );
    let _guard = _span.enter();

    db.delete(as_key(&PREFIX, key)).map_err(|err| StoreError::Internal(err.into()))
}
