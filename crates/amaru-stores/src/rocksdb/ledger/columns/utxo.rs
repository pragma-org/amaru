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

use crate::rocksdb::common::{PREFIX_LEN, as_key, as_value};
use amaru_kernel::cbor;
use amaru_ledger::store::{
    StoreError,
    columns::utxo::{Key, Value},
};
use rocksdb::{DBPinnableSlice, Transaction};

/// Name prefixed used for storing UTxO entries. UTF-8 encoding for "utxo"
pub const PREFIX: [u8; PREFIX_LEN] = [0x75, 0x74, 0x78, 0x6f];

#[expect(clippy::panic)]
pub fn get<'a>(
    db_get: impl Fn(&[u8]) -> Result<Option<DBPinnableSlice<'a>>, rocksdb::Error>,
    key: &Key,
) -> Result<Option<Value>, StoreError> {
    let key = as_key(&PREFIX, key);
    let bytes = db_get(&key);
    Ok(bytes
        .map_err(|err| StoreError::Internal(err.into()))?
        .map(|bytes| {
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
    rows: impl Iterator<Item = (Key, Value)>,
) -> Result<(), StoreError> {
    for (input, output) in rows {
        db.put(as_key(&PREFIX, input), as_value(output))
            .map_err(|err| StoreError::Internal(err.into()))?;
    }

    Ok(())
}

pub fn remove<DB>(
    db: &Transaction<'_, DB>,
    rows: impl Iterator<Item = Key>,
) -> Result<(), StoreError> {
    for input in rows {
        db.delete(as_key(&PREFIX, input))
            .map_err(|err| StoreError::Internal(err.into()))?;
    }

    Ok(())
}
