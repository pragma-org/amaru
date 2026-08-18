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

use std::collections::BTreeMap;

use amaru_ledger::store::{
    StoreError,
    columns::{
        pools_vrf::{Key, Value},
        unsafe_decode,
    },
};
use amaru_observability::{error, trace_span};
use rocksdb::{DBPinnableSlice, Transaction};

use crate::rocksdb::{
    as_value,
    common::{PREFIX_LEN, as_key},
};

/// Name prefixed used for storing VRF key hash occupancy entries.
pub const PREFIX: [u8; PREFIX_LEN] = *b"pvrf";

/// Point-read the occupancy count of a VRF key hash, if in use.
pub fn get<'a>(
    db_get: impl Fn(&[u8]) -> Result<Option<DBPinnableSlice<'a>>, rocksdb::Error>,
    vrf: &Key,
) -> Result<Option<Value>, StoreError> {
    trace_span!(stores::ledger::pools_vrf::GET).in_scope(|| {
        let key = as_key(&PREFIX, vrf);
        Ok(db_get(&key).map_err(|err| StoreError::Internal(err.into()))?.map(|d| unsafe_decode::<Value>(&d)))
    })
}

/// Mark a VRF key hash as in use by a pool registration. This *sets* the count to 1 even when an
/// entry already exists; it never increments.
pub fn claim<DB>(db: &Transaction<'_, DB>, vrf: &Key) -> Result<(), StoreError> {
    trace_span!(stores::ledger::pools_vrf::CLAIM).in_scope(|| {
        db.put(as_key(&PREFIX, vrf), as_value(1 as Value)).map_err(|err| StoreError::Internal(err.into()))?;
        Ok(())
    })
}

/// Delete an entry whole-key, when the VRF key hash it tracks has been superseded by a differing
/// one (an in-window re-registration, or a dangling key at the epoch boundary).
pub fn release<DB>(db: &Transaction<'_, DB>, vrf: &Key) -> Result<(), StoreError> {
    trace_span!(stores::ledger::pools_vrf::RELEASE).in_scope(|| {
        let key = as_key(&PREFIX, vrf);
        match db.get(&key).map_err(|err| StoreError::Internal(err.into()))? {
            None => {
                error!(stores::ledger::pools_vrf::RELEASE, ?vrf, reason = "vrf key hash not in use");
            }
            Some(..) => db.delete(key).map_err(|err| StoreError::Internal(err.into()))?,
        }

        Ok(())
    })
}

/// Decrement the occupancy count of a retiring pool's VRF key hash, dropping the entry at zero.
/// An absent key means the stored count stood below the number of pools holding it.
pub fn decrement<DB>(db: &Transaction<'_, DB>, vrf: &Key) -> Result<(), StoreError> {
    trace_span!(stores::ledger::pools_vrf::DECREMENT).in_scope(|| {
        let key = as_key(&PREFIX, vrf);
        match db.get(&key).map_err(|err| StoreError::Internal(err.into()))? {
            None => {
                error!(stores::ledger::pools_vrf::DECREMENT, ?vrf, reason = "vrf key hash not in use");
            }
            Some(bytes) => {
                let count = unsafe_decode::<Value>(&bytes);
                if count <= 1 {
                    db.delete(key).map_err(|err| StoreError::Internal(err.into()))?;
                } else {
                    db.put(key, as_value(count - 1)).map_err(|err| StoreError::Internal(err.into()))?;
                }
            }
        }

        Ok(())
    })
}

/// Import an occupancy entry verbatim from a node snapshot. Bootstrap-only: counts above 1 cannot
/// be reconstructed from registrations, so the snapshot's map is copied as-is.
pub fn seed<DB>(db: &Transaction<'_, DB>, vrf: &Key, count: Value) -> Result<(), StoreError> {
    trace_span!(stores::ledger::pools_vrf::SEED).in_scope(|| {
        db.put(as_key(&PREFIX, vrf), as_value(count)).map_err(|err| StoreError::Internal(err.into()))?;
        Ok(())
    })
}

/// Clear the column, then set every key in `counts` to its occupancy count.
pub fn import<DB>(db: &Transaction<'_, DB>, counts: &BTreeMap<Key, Value>) -> Result<(), StoreError> {
    let stale = db
        .prefix_iterator(PREFIX)
        .map(|entry| entry.map(|(key, _)| key).map_err(|err| StoreError::Internal(err.into())))
        .collect::<Result<Vec<_>, _>>()?;

    for key in stale {
        db.delete(key).map_err(|err| StoreError::Internal(err.into()))?;
    }

    for (vrf, count) in counts {
        seed(db, vrf, *count)?;
    }

    Ok(())
}
