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
    columns::utxo::{Key, Value},
    StoreError,
};
use pallas_codec::minicbor::{self as cbor};
use rocksdb::{OptimisticTransactionDB, ThreadMode, Transaction};

/// Name prefixed used for storing UTxO entries. UTF-8 encoding for "utxo"
pub const PREFIX: [u8; PREFIX_LEN] = [0x75, 0x74, 0x78, 0x6f];

#[allow(clippy::panic)]
pub fn get<T: ThreadMode>(
    db: &OptimisticTransactionDB<T>,
    key: &Key,
) -> Result<Option<Value>, StoreError> {
    Ok(db
        .get(as_key(&PREFIX, key))
        .map_err(|err| StoreError::Internal(err.into()))?
        .map(|bytes| {
            let output = cbor::decode(&bytes).unwrap_or_else(|e| {
                panic!(
                    "unable to decode TransactionOutput from CBOR ({}): {e:?}",
                    hex::encode(&bytes)
                )
            });
            Value(into_owned_output(output))
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
