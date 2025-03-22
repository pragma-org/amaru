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
use amaru_ledger::store::StoreError;
use rocksdb::Transaction;

use amaru_ledger::store::columns::cc_members::{Key, Row, Value};

/// Name prefixed used for storing delegations entries. UTF-8 encoding for "comm"
pub const PREFIX: [u8; PREFIX_LEN] = [0x43, 0x4F, 0x4D, 0x4D];

/// Register a new CC Member.
pub fn add<DB>(
    db: &Transaction<'_, DB>,
    rows: impl Iterator<Item = (Key, Value)>,
) -> Result<(), StoreError> {
    for (credential, hot_credential) in rows {
        let key = as_key(&PREFIX, &credential);

        // In case where a registration already exists, then we must only update the underlying
        // entry.
        let mut row = db
            .get(&key)
            .map_err(|err| StoreError::Internal(err.into()))?
            .map(Row::unsafe_decode)
            .unwrap_or(Row {
                hot_credential: None,
            });

        hot_credential.set_or_reset(&mut row.hot_credential);

        db.put(key, as_value(row))
            .map_err(|err| StoreError::Internal(err.into()))?;
    }

    Ok(())
}
