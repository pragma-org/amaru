// Copyright 2025 PRAGMA
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
use rocksdb::Transaction;
use std::ops::Deref;

pub use amaru_ledger::store::{
    columns::proposals::{Key, Row, Value},
    StoreError,
};

/// Name prefixed used for storing Proposals entries. UTF-8 encoding for "prop"
pub const PREFIX: [u8; PREFIX_LEN] = [0x70, 0x72, 0x6F, 0x70];

/// Register a new Proposal.
#[allow(clippy::unwrap_used)]
pub fn add<DB>(
    db: &Transaction<'_, DB>,
    rows: impl Iterator<Item = (Key, Value)>,
) -> Result<(), StoreError> {
    for (key, value) in rows {
        db.put(as_key(&PREFIX, key), as_value(value))
            .map_err(|err| StoreError::Internal(err.into()))?;
    }

    Ok(())
}

/// Remove an expired or enacted proposal.
pub fn remove<'iter, DB, K>(
    db: &Transaction<'_, DB>,
    rows: impl Iterator<Item = K>,
) -> Result<(), StoreError>
where
    K: Deref<Target = Key> + 'iter,
{
    for key in rows {
        db.delete(as_key(&PREFIX, key.deref()))
            .map_err(|err| StoreError::Internal(err.into()))?;
    }

    Ok(())
}
