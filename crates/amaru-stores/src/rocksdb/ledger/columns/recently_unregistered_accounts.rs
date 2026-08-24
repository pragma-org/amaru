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

use amaru_kernel::Epoch;
use amaru_ledger::store::{
    StoreError,
    columns::recently_unregistered_accounts::{Key, Value},
};
use amaru_observability::trace_span;
use rocksdb::Transaction;

use crate::rocksdb::{
    as_value,
    common::{PREFIX_LEN, as_key},
    with_prefix_iterator,
};

/// Name prefixed used for storing recently unregistered accounts.
pub const PREFIX: [u8; PREFIX_LEN] = *b"ruac";
pub const COLLECTION_NAME: &str = "recently_unregistered_accounts";

/// Insert a single entry
pub fn insert<DB>(db: &Transaction<'_, DB>, key: &Key, value: Value) -> Result<(), StoreError> {
    trace_span!(stores::ledger::recently_unregistered_accounts::INSERT).in_scope(|| {
        db.put(as_key(&PREFIX, key), as_value(value)).map_err(|err| StoreError::Internal(err.into()))?;
        Ok(())
    })
}

/// Remove one entry, for example, when accounts are being re-registered.
pub fn remove<DB>(db: &Transaction<'_, DB>, key: &Key) -> Result<(), StoreError> {
    trace_span!(stores::ledger::recently_unregistered_accounts::REMOVE).in_scope(|| {
        db.delete(as_key(&PREFIX, key)).map_err(|err| StoreError::Internal(err.into()))?;
        Ok(())
    })
}

/// Remove all entries older than a certain epoch. We only need to remember recently pruned accounts
/// for a few epochs and can prune old de-registrations once they're no longer relevant to rewards
/// application. Consider the following:
///
/// - in epoch `e`, rewards are calculated using the stake distribution from the end of `e-3`
/// - in the transition from `e` to `e+1`, we need to know which accounts have since unregistered
///   and cannot receive rewards.
/// - at the beginning of `e+1`, call this method and prune old data.
///
/// So any de-registration from `e-2`, `e-1` or `e` must survive until the end of `e`. Then, `e+2`
/// can be cleared at the start of the next epoch `e+1`.
pub fn prune<DB>(db: &Transaction<'_, DB>, epoch: Epoch) -> Result<(), StoreError> {
    trace_span!(stores::ledger::recently_unregistered_accounts::PRUNE, epoch).in_scope(|| {
        with_prefix_iterator::<Key, Value, DB>(db, PREFIX, COLLECTION_NAME, |iterator| {
            for (_, mut row) in iterator {
                let value = row.borrow_mut();
                if value.is_some_and(|unregistered_at| unregistered_at + 3 <= epoch) {
                    *value = None;
                }
            }
        })?;

        Ok(())
    })
}
