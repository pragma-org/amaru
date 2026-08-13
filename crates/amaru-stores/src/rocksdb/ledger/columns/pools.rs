// Copyright 2024 PRAGMA
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

use std::collections::{BTreeMap, BTreeSet};

use amaru_kernel::Epoch;
use amaru_ledger::{
    epoch_transition::pools_updates::{PoolCertificate, PoolCertificates},
    store::{
        StoreError,
        columns::{
            pools::{Key, Row, Value},
            pools_vrf::Key as VrfKey,
            unsafe_decode,
        },
    },
};
use amaru_observability::{error, trace_span};
use rocksdb::{DBPinnableSlice, Transaction};

use crate::rocksdb::{
    common::{PREFIX_LEN, as_key, as_value},
    pools_vrf,
};

/// Name prefixed used for storing Pool entries. UTF-8 encoding for "pool"
pub const PREFIX: [u8; PREFIX_LEN] = [0x70, 0x6f, 0x6f, 0x6c];

pub fn get<'a>(
    db_get: impl Fn(&[u8]) -> Result<Option<DBPinnableSlice<'a>>, rocksdb::Error>,
    pool: &Key,
) -> Result<Option<Row>, StoreError> {
    trace_span!(stores::ledger::pools::GET).in_scope(|| {
        let key = as_key(&PREFIX, pool);
        Ok(db_get(&key).map_err(|err| StoreError::Internal(err.into()))?.map(|d| unsafe_decode::<Row>(&d)))
    })
}

pub fn add<DB>(db: &Transaction<'_, DB>, rows: impl Iterator<Item = Value>, epoch: Epoch) -> Result<(), StoreError> {
    trace_span!(stores::ledger::pools::ADD).in_scope(|| {
        for (params, registered_at, deposit) in rows {
            let pool = params.id;

            // Pool parameters are stored in an epoch-aware fashion.
            //
            // - If no parameters exist for the pool, we can immediately create a new
            //   entry.
            //
            // - If one already exists, then the parameters are stashed until the next
            //   epoch boundary.
            //
            // Either way, the registration claims its VRF key hash occupancy, and a
            // re-registration frees the key of any pending registration it supersedes
            // when that key differed.
            //
            // TODO: We might want to define a MERGE OPERATOR to speed this up if
            // necessary.
            let params = match db.get(as_key(&PREFIX, pool)).map_err(|err| StoreError::Internal(err.into()))? {
                None => {
                    pools_vrf::claim(db, &params.vrf)?;

                    as_value(Row {
                        registered_at,
                        deposit,
                        current_params: params,
                        pending_certificates: PoolCertificates::default(),
                    })
                }
                Some(existing_params) => {
                    pools_vrf::claim(db, &params.vrf)?;

                    // The row may still hold certificates whose epoch boundary has passed but
                    // which have not been ticked yet; folding them at the current epoch applies
                    // the boundary cancellation rules. In particular, a retirement that is
                    // already effective discards the pending registration, whose VRF is then
                    // netted out by the epoch-boundary purge rather than released here.
                    let row = unsafe_decode::<Row>(&existing_params);
                    if let Some(pending) = row.pending_certificates.pending_after(epoch).registration()
                        && pending.vrf != params.vrf
                    {
                        pools_vrf::release(db, &pending.vrf)?;
                    }

                    Row::extend(existing_params, PoolCertificate::from(params))
                }
            };

            db.put(as_key(&PREFIX, pool), params).map_err(|err| StoreError::Internal(err.into()))?;
        }

        Ok(())
    })
}

/// Apply the pool updates and retirements computed at an epoch boundary, together with the VRF
/// key hash occupancy changes they imply: whole-key deletes of superseded ("dangling") keys
/// first, then a decrement per retiring pool's post-activation key.
pub fn update_or_retire<DB>(
    db: &Transaction<'_, DB>,
    updates: &BTreeMap<Key, Row>,
    retirements: &BTreeSet<Key>,
    vrf_released: &BTreeSet<VrfKey>,
    vrf_retired: &[VrfKey],
) -> Result<(), StoreError> {
    trace_span!(stores::ledger::pools::UPDATE_OR_RETIRE).in_scope(|| {
        for (pool, row) in updates {
            db.put(as_key(&PREFIX, pool), as_value(row)).map_err(|err| StoreError::Internal(err.into()))?;
        }

        for pool in retirements {
            db.delete(as_key(&PREFIX, pool)).map_err(|err| StoreError::Internal(err.into()))?;
        }

        for vrf in vrf_released {
            pools_vrf::release(db, vrf)?;
        }

        for vrf in vrf_retired {
            pools_vrf::decrement(db, vrf)?;
        }

        Ok(())
    })
}

pub fn remove<DB>(db: &Transaction<'_, DB>, rows: impl Iterator<Item = (Key, Epoch)>) -> Result<(), StoreError> {
    trace_span!(stores::ledger::pools::REMOVE).in_scope(|| {
        for (pool, epoch) in rows {
            // We do not delete pool immediately but rather schedule the
            // removal as an empty parameter update. The 'pool reaping' happens on
            // every epoch boundary.
            match db.get(as_key(&PREFIX, pool)).map_err(|err| StoreError::Internal(err.into()))? {
                None => {
                    error!(stores::ledger::pools::REMOVE, ?pool, reason = "unknown pool");
                }
                Some(existing_params) => db
                    .put(as_key(&PREFIX, pool), Row::extend(existing_params, PoolCertificate::Retirement(epoch)))
                    .map_err(|err| StoreError::Internal(err.into()))?,
            };
        }

        Ok(())
    })
}
