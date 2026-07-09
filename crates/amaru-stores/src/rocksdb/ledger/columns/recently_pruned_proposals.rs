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

pub use amaru_ledger::store::{
    StoreError,
    columns::recently_pruned_proposals::{Key, Value},
};
use amaru_observability::debug_span;
use rocksdb::Transaction;

use crate::rocksdb::{
    as_value,
    common::{PREFIX_LEN, as_key},
    with_prefix_iterator,
};

/// Name prefixed used for storing recently pruned proposals entries.
pub const PREFIX: [u8; PREFIX_LEN] = *b"prup";

pub const COLLECTION_NAME: &str = "recently_pruned_proposals";

/// Replace all recently pruned proposals with a new set.
pub fn replace_all<'iter, DB>(
    db: &Transaction<'_, DB>,
    rows: impl IntoIterator<Item = (&'iter Key, Value)>,
) -> Result<(), StoreError> {
    debug_span!(
        amaru_observability::amaru::stores::ledger::columns::RECENTLY_PRUNED_PROPOSALS_REPLACE_ALL,
        db_system_name = "rocksdb".to_string(),
        db_operation_name = "put".to_string(),
        db_collection_name = COLLECTION_NAME.to_string()
    )
    .in_scope(|| {
        with_prefix_iterator::<Key, Value, DB>(db, PREFIX, COLLECTION_NAME, |iterator| {
            for (_, mut row) in iterator {
                *row.borrow_mut() = None;
            }
        })?;

        for (key, value) in rows {
            db.put(as_key(&PREFIX, key), as_value(value)).map_err(|err| StoreError::Internal(err.into()))?;
        }

        Ok(())
    })
}
