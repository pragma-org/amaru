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
use amaru_kernel::{StakeCredential, Voter};
use amaru_ledger::store::{
    columns::votes::{Key, Value},
    StoreError,
};
use std::collections::BTreeSet;

pub fn add(
    store: &MemoryStore,
    rows: impl Iterator<Item = (Key, Value)>,
) -> Result<BTreeSet<StakeCredential>, StoreError> {
    let mut voting_dreps = BTreeSet::new();

    for (key, value) in rows {
        match key.voter {
            Voter::DRepKey(hash) => {
                voting_dreps.insert(StakeCredential::AddrKeyhash(hash));
            }
            Voter::DRepScript(hash) => {
                voting_dreps.insert(StakeCredential::ScriptHash(hash));
            }
            Voter::ConstitutionalCommitteeKey(..)
            | Voter::ConstitutionalCommitteeScript(..)
            | Voter::StakePoolKey(..) => {}
        }

        store.votes.borrow_mut().insert(key, value);
    }

    Ok(voting_dreps)
}
