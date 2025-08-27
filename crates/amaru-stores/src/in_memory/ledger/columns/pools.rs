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

use crate::in_memory::MemoryStore;
use amaru_ledger::store::{
    StoreError,
    columns::pools::{Key, Row, Value},
};
use amaru_slot_arithmetic::Epoch;
use tracing::error;

pub fn add(
    store: &MemoryStore,
    rows: impl Iterator<Item = Value>, // Value = (PoolParams, CertificatePointer, Epoch)
) -> Result<(), StoreError> {
    let mut pools = store.pools.borrow_mut();

    for (pool_params, registered_at, epoch) in rows {
        let key = pool_params.id;

        if let Some(row) = pools.get_mut(&key) {
            row.future_params.push((Some(pool_params), epoch));
        } else {
            let row = Row::new(registered_at, pool_params);
            pools.insert(key, row);
        }
    }

    Ok(())
}

pub fn remove(
    store: &MemoryStore,
    rows: impl Iterator<Item = (Key, Epoch)>,
) -> Result<(), StoreError> {
    let mut pools = store.pools.borrow_mut();

    for (pool_id, epoch) in rows {
        match pools.get_mut(&pool_id) {
            Some(row) => {
                row.future_params.push((None, epoch));
            }
            None => {
                error!(target: "store::pools::remove", ?pool_id, "remove.unknown");
            }
        }
    }

    Ok(())
}
