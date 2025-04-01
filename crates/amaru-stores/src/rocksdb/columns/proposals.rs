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
    for (proposal_pointer, row) in rows {
        let key = as_key(&PREFIX, proposal_pointer);
        db.put(key, as_value(row))
            .map_err(|err| StoreError::Internal(err.into()))?;
    }

    Ok(())
}

/// Clear a Proposal registration.
pub fn remove<DB>(
    db: &Transaction<'_, DB>,
    rows: impl Iterator<Item = Key>,
) -> Result<(), StoreError> {
    for proposal_pointer in rows {
        db.delete(as_key(&PREFIX, proposal_pointer))
            .map_err(|err| StoreError::Internal(err.into()))?;
    }

    Ok(())
}
