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

use amaru_ledger::store::{
    StoreError,
    columns::{
        unsafe_decode,
        vrf_keys::{DiffVrf, Key},
    },
};
use amaru_observability::{error, trace_span};
use rocksdb::{DBPinnableSlice, Transaction};

use crate::rocksdb::{
    as_value,
    common::{PREFIX_LEN, as_key},
};

/// Name prefixed used for storing VRF key hash occupancy entries.
pub const PREFIX: [u8; PREFIX_LEN] = *b"vrfs";

/// Point-read the occupancy count of a VRF key hash, if in use.
pub fn get<'a>(
    db_get: impl Fn(&[u8]) -> Result<Option<DBPinnableSlice<'a>>, rocksdb::Error>,
    vrf: &Key,
) -> Result<Option<u64>, StoreError> {
    trace_span!(stores::ledger::vrf_keys::GET).in_scope(|| {
        let key = as_key(&PREFIX, vrf);
        Ok(db_get(&key).map_err(|err| StoreError::Internal(err.into()))?.map(|d| unsafe_decode::<u64>(&d)))
    })
}

/// Apply an update to a VRF key.
pub fn update<DB>(db: &Transaction<'_, DB>, vrf: &Key, diff: DiffVrf) -> Result<(), StoreError> {
    trace_span!(stores::ledger::vrf_keys::UPDATE).in_scope(|| {
        let key = as_key(&PREFIX, vrf);
        match diff {
            DiffVrf::Claim => db.put(key, as_value(1_u64)),
            DiffVrf::Release => db.delete(key),
            DiffVrf::Decrement(by) => match db.get(&key).map_err(|err| StoreError::Internal(err.into()))? {
                None => {
                    error!(
                        stores::ledger::vrf_keys::DECREMENT,
                        ?vrf,
                        by = by,
                        stored = 0,
                        error = "vrf key hash not in use"
                    );
                    return Ok(());
                }
                Some(bytes) => {
                    let stored = unsafe_decode::<u64>(&bytes);
                    match stored.checked_sub(by).filter(|remaining| *remaining > 0) {
                        Some(remaining) => db.put(key, as_value(remaining)),
                        None => db.delete(key),
                    }
                }
            },
        }
        .map_err(|err| StoreError::Internal(err.into()))
    })
}
