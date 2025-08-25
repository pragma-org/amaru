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
use amaru_ledger::store::{
    StoreError,
    columns::{
        pools::{EVENT_TARGET, Key, Row, Value},
        unsafe_decode,
    },
};
use amaru_slot_arithmetic::Epoch;
use rocksdb::Transaction;
use tracing::error;

/// Name prefixed used for storing Pool entries. UTF-8 encoding for "pool"
pub const PREFIX: [u8; PREFIX_LEN] = [0x70, 0x6f, 0x6f, 0x6c];

pub fn get(
    db_get: impl Fn(&[u8]) -> Result<Option<Vec<u8>>, rocksdb::Error>,
    pool: &Key,
) -> Result<Option<Row>, StoreError> {
    let key = as_key(&PREFIX, pool);
    Ok(db_get(&key)
        .map_err(|err| StoreError::Internal(err.into()))?
        .map(unsafe_decode::<Row>))
}

pub fn add<DB>(
    db: &Transaction<'_, DB>,
    rows: impl Iterator<Item = Value>,
) -> Result<(), StoreError> {
    for (params, epoch) in rows {
        let pool = params.id;

        // Pool parameters are stored in an epoch-aware fashion.
        //
        // - If no parameters exist for the pool, we can immediately create a new
        //   entry.
        //
        // - If one already exists, then the parameters are stashed until the next
        //   epoch boundary.
        //
        // TODO: We might want to define a MERGE OPERATOR to speed this up if
        // necessary.
        let params = match db
            .get(as_key(&PREFIX, pool))
            .map_err(|err| StoreError::Internal(err.into()))?
        {
            None => as_value(Row::new(params)),
            Some(existing_params) => Row::extend(existing_params, (Some(params), epoch)),
        };

        db.put(as_key(&PREFIX, pool), params)
            .map_err(|err| StoreError::Internal(err.into()))?;
    }

    Ok(())
}

pub fn remove<DB>(
    db: &Transaction<'_, DB>,
    rows: impl Iterator<Item = (Key, Epoch)>,
) -> Result<(), StoreError> {
    for (pool, epoch) in rows {
        // We do not delete pool immediately but rather schedule the
        // removal as an empty parameter update. The 'pool reaping' happens on
        // every epoch boundary.
        match db
            .get(as_key(&PREFIX, pool))
            .map_err(|err| StoreError::Internal(err.into()))?
        {
            None => {
                error!(target: EVENT_TARGET, ?pool, "remove.unknown")
            }
            Some(existing_params) => db
                .put(
                    as_key(&PREFIX, pool),
                    Row::extend(existing_params, (None, epoch)),
                )
                .map_err(|err| StoreError::Internal(err.into()))?,
        };
    }

    Ok(())
}
