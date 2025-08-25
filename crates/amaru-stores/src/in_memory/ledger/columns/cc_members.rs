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
use amaru_ledger::{
    state::diff_bind::Resettable,
    store::{
        StoreError,
        columns::cc_members::{Key, Row, Value},
    },
};

pub fn add(
    store: &MemoryStore,
    rows: impl Iterator<Item = (Key, Value)>,
) -> Result<(), StoreError> {
    let mut cc_members = store.cc_members.borrow_mut();

    for (key, (hot_credential, valid_until)) in rows {
        let row = cc_members.get(&key).cloned().or(match valid_until {
            Resettable::Set(valid_until) => Some(Row {
                hot_credential: None,
                valid_until,
            }),
            Resettable::Unchanged | Resettable::Reset => None,
        });

        if let Some(mut row) = row {
            hot_credential.set_or_reset(&mut row.hot_credential);
            cc_members.insert(key, row);
        }
    }

    Ok(())
}

pub fn remove(store: &MemoryStore, rows: impl Iterator<Item = Key>) -> Result<(), StoreError> {
    let mut cc_members = store.cc_members.borrow_mut();

    for key in rows {
        cc_members.remove(&key);
    }

    Ok(())
}
