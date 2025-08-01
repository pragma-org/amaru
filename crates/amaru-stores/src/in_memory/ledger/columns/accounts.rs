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
use amaru_kernel::StakeCredential;
use amaru_ledger::store::{
    columns::accounts::{Key, Row, Value},
    StoreError,
};
use tracing::error;

pub fn add(
    store: &MemoryStore,
    rows: impl Iterator<Item = (Key, Value)>,
) -> Result<(), StoreError> {
    let mut accounts = store.accounts.borrow_mut();

    for (key, (delegatee, drep, rewards, deposit)) in rows {
        let mut row = accounts.get(&key).cloned().unwrap_or(Row {
            delegatee: None,
            drep: None,
            rewards: 0,
            deposit: 0,
        });

        delegatee.set_or_reset(&mut row.delegatee);
        drep.set_or_reset(&mut row.drep);

        if let Some(r) = rewards {
            row.rewards = r;
        }

        row.deposit = deposit;

        accounts.insert(key, row);
    }

    Ok(())
}

pub fn reset_many(
    store: &MemoryStore,
    rows: impl Iterator<Item = StakeCredential>,
) -> Result<(), StoreError> {
    let mut accounts = store.accounts.borrow_mut();

    for credential in rows {
        match accounts.get_mut(&credential) {
            Some(row) => {
                row.rewards = 0;
            }
            None => {
                error!(
                    target: "store::accounts::reset_many",
                    ?credential,
                    "reset.no_account",
                );
            }
        }
    }

    Ok(())
}

pub fn remove(
    store: &MemoryStore,
    rows: impl Iterator<Item = StakeCredential>,
) -> Result<(), StoreError> {
    let mut accounts = store.accounts.borrow_mut();

    for credential in rows {
        accounts.remove(&credential);
    }

    Ok(())
}
