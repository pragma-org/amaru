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

use crate::rocksdb::common::{PREFIX_LEN, as_key, as_value};
use amaru_ledger::store::{StoreError, columns::unsafe_decode};
use rocksdb::Transaction;

use amaru_ledger::store::columns::cc_members::{Key, Row, Value};

/// Name prefixed used for storing delegations entries. UTF-8 encoding for "comm"
pub const PREFIX: [u8; PREFIX_LEN] = [0x43, 0x4F, 0x4D, 0x4D];

/// Register a new CC Member.
pub fn upsert<DB>(
    db: &Transaction<'_, DB>,
    rows: impl Iterator<Item = (Key, Value)>,
) -> Result<(), StoreError> {
    for (cold_credential, (hot_credential, valid_until)) in rows {
        let key = as_key(&PREFIX, &cold_credential);

        let mut row = db
            .get_pinned(&key)
            .map_err(|err| StoreError::Internal(err.into()))?
            .map(|d| unsafe_decode::<Row>(&d))
            // NOTE:
            // (1) If the registration doesn't exists, but a new cc member is being added,
            // then we can initialize a default value.
            //
            // (2) unelected-but-potential (i.e. present in ongoing proposals) CC members are *allowed*
            // to declare their hot/cold delegation. Unelected CC are conserved as being valid
            // until epoch 0.
            .unwrap_or_else(|| Row {
                hot_credential: None,
                valid_until: None,
            });

        valid_until.set_or_reset(&mut row.valid_until);
        hot_credential.set_or_reset(&mut row.hot_credential);

        db.put(key, as_value(row))
            .map_err(|err| StoreError::Internal(err.into()))?;
    }

    Ok(())
}
