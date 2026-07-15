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

use std::ops::Deref;

use amaru_kernel::ProposalId;
pub use amaru_ledger::store::{
    StoreError,
    columns::{
        proposals::{Key, Row, Value},
        unsafe_decode,
    },
};
use amaru_observability::trace_span;
use rocksdb::{DBPinnableSlice, Transaction};

use crate::rocksdb::common::{PREFIX_LEN, as_key, as_value};

/// Name prefixed used for storing Proposals entries. UTF-8 encoding for "prop"
pub const PREFIX: [u8; PREFIX_LEN] = *b"prop";

/// Retrieve a single governance proposal.
pub fn get<'a>(
    db_get: impl Fn(&[u8]) -> Result<Option<DBPinnableSlice<'a>>, rocksdb::Error>,
    id: &Key,
) -> Result<Option<Row>, StoreError> {
    let _span = trace_span!(
        stores::ledger::columns::PROPOSALS_GET,
        db_system_name = "rocksdb".to_string(),
        db_operation_name = "get".to_string(),
        db_collection_name = "proposals".to_string()
    );
    let _guard = _span.enter();

    let key = as_key(&PREFIX, id);
    let bytes = db_get(&key);
    bytes.map_err(|err| StoreError::Internal(err.into())).map(|opt| opt.map(|d| unsafe_decode::<Row>(&d)))
}

/// Register a new Proposal.
pub fn add<DB>(db: &Transaction<'_, DB>, rows: impl Iterator<Item = (Key, Value)>) -> Result<usize, StoreError> {
    trace_span!(
        stores::ledger::columns::PROPOSALS_ADD,
        db_system_name = "rocksdb".to_string(),
        db_operation_name = "write".to_string(),
        db_collection_name = "proposals".to_string()
    )
    .in_scope(|| {
        let mut n = 0;

        for (key, value) in rows {
            n += 1;
            db.put(as_key(&PREFIX, key), as_value(value)).map_err(|err| StoreError::Internal(err.into()))?;
        }

        Ok(n)
    })
}

/// Remove an expired or enacted proposal.
pub fn remove<'iter, DB, K>(db: &Transaction<'_, DB>, rows: impl Iterator<Item = &'iter K>) -> Result<(), StoreError>
where
    K: Deref<Target = ProposalId> + 'iter,
{
    trace_span!(
        stores::ledger::columns::PROPOSALS_REMOVE,
        db_system_name = "rocksdb".to_string(),
        db_operation_name = "delete".to_string(),
        db_collection_name = "proposals".to_string()
    )
    .in_scope(|| {
        for key in rows {
            db.delete(as_key(&PREFIX, key.deref())).map_err(|err| StoreError::Internal(err.into()))?;
        }

        Ok(())
    })
}
