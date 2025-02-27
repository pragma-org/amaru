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

use amaru_ledger::store::columns::delegations::{Key, Row, Value};

/// Name prefixed used for storing delegations entries. UTF-8 encoding for "delg"
pub const PREFIX: [u8; PREFIX_LEN] = [0x64, 0x65, 0x6C, 0x67];

/// Register a new DRep.
pub fn add<DB>(
    db: &Transaction<'_, DB>,
    rows: impl Iterator<Item = (Key, Value)>,
) -> Result<(), StoreError> {
    for (credential, drep) in rows {
        let key = as_key(&PREFIX, &credential);
        // Always override if a mapping already exists.
        let row = Row { drep };
        db.put(key, as_value(row))
            .map_err(|err| StoreError::Internal(err.into()))?;
    }

    Ok(())
}
