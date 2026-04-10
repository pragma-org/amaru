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

use std::collections::BTreeSet;

use amaru_kernel::{StakeCredential, Voter};
pub use amaru_ledger::store::{
    StoreError,
    columns::votes::{Key, Row, Value},
};
use amaru_observability::trace_span;
use rocksdb::Transaction;

use crate::rocksdb::common::{PREFIX_LEN, as_key, as_value};

/// Name prefixed used for storing Proposals entries. UTF-8 encoding for "vote"
pub const PREFIX: [u8; PREFIX_LEN] = [0x76, 0x6f, 0x74, 0x65];

/// Register a series of new votes. Returns the credentials (script or key) of all dreps found
/// amongst the voters.
pub fn add<DB>(
    db: &Transaction<'_, DB>,
    rows: impl Iterator<Item = (Key, Value)>,
) -> Result<BTreeSet<StakeCredential>, StoreError> {
    let _span = trace_span!(
        amaru_observability::amaru::stores::ledger::columns::VOTES_ADD,
        db_system_name = "rocksdb".to_string(),
        db_operation_name = "write".to_string(),
        db_collection_name = "vote".to_string()
    );
    let _guard = _span.enter();

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

        db.put(as_key(&PREFIX, &key), as_value(value)).map_err(|err| StoreError::Internal(err.into()))?;
    }

    Ok(voting_dreps)
}
