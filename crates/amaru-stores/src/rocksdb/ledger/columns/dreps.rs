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

use amaru_kernel::{Epoch, StakeCredential};
use amaru_ledger::store::{
    StoreError,
    columns::{
        dreps::{EVENT_TARGET, Key, Row, Value},
        unsafe_decode,
    },
};
use amaru_observability::trace_span;
use rocksdb::{DBPinnableSlice, Transaction};
use tracing::{error, warn};

use crate::rocksdb::common::{PREFIX_LEN, as_key, as_value};

/// Name prefixed used for storing DReps entries. UTF-8 encoding for "drep"
pub const PREFIX: [u8; PREFIX_LEN] = [0x64, 0x72, 0x65, 0x70];

/// Retrieve a single DRep
pub fn get<'a>(
    db_get: impl Fn(&[u8]) -> Result<Option<DBPinnableSlice<'a>>, rocksdb::Error>,
    credential: &Key,
) -> Result<Option<Row>, StoreError> {
    let _span = trace_span!(
        amaru_observability::amaru::stores::ledger::columns::DREPS_GET,
        db_system_name = "rocksdb".to_string(),
        db_operation_name = "get".to_string(),
        db_collection_name = "drep".to_string()
    );
    let _guard = _span.enter();

    let key = as_key(&PREFIX, credential);
    Ok(db_get(&key).map_err(|err| StoreError::Internal(err.into()))?.map(|d| unsafe_decode::<Row>(&d)))
}

/// Persist a DRep's materialized row, overwriting any previous entry.
pub fn add<DB>(db: &Transaction<'_, DB>, rows: impl Iterator<Item = (Key, Value)>) -> Result<(), StoreError> {
    let _span = trace_span!(
        amaru_observability::amaru::stores::ledger::columns::DREPS_ADD,
        db_system_name = "rocksdb".to_string(),
        db_operation_name = "write".to_string(),
        db_collection_name = "drep".to_string()
    );
    let _guard = _span.enter();

    for (credential, row) in rows {
        db.put(as_key(&PREFIX, &credential), as_value(row)).map_err(|err| StoreError::Internal(err.into()))?;
    }

    Ok(())
}

/// Re-calculate drep expiry based the current epoch. This happens each time a drep vote on an
/// active governance proposal.
pub fn set_valid_until<DB>(
    db: &Transaction<'_, DB>,
    credentials: BTreeSet<StakeCredential>,
    valid_until: Epoch,
) -> Result<(), StoreError> {
    let _span = trace_span!(
        amaru_observability::amaru::stores::ledger::columns::DREPS_SET_VALID_UNTIL,
        db_system_name = "rocksdb".to_string(),
        db_operation_name = "write".to_string(),
        db_collection_name = "drep".to_string()
    );
    let _guard = _span.enter();

    for credential in credentials {
        let key = as_key(&PREFIX, &credential);

        if let Some(mut row) =
            db.get_pinned(&key).map_err(|err| StoreError::Internal(err.into()))?.map(|d| unsafe_decode::<Row>(&d))
        {
            row.valid_until = valid_until;
            db.put(key, as_value(row)).map_err(|err| StoreError::Internal(err.into()))?;
        } else {
            warn!(
                target: EVENT_TARGET,
                ?credential,
                "set_valid_until.unknown_drep",
            )
        };
    }

    Ok(())
}

/// Clear a DRep registration.
pub fn remove<DB>(db: &Transaction<'_, DB>, rows: impl Iterator<Item = Key>) -> Result<(), StoreError> {
    let _span = trace_span!(
        amaru_observability::amaru::stores::ledger::columns::DREPS_REMOVE,
        db_system_name = "rocksdb".to_string(),
        db_operation_name = "write".to_string(),
        db_collection_name = "drep".to_string()
    );
    let _guard = _span.enter();

    for drep in rows {
        let key = as_key(&PREFIX, &drep);

        if db.get_pinned(&key).map_err(|err| StoreError::Internal(err.into()))?.is_some() {
            db.delete(key).map_err(|err| StoreError::Internal(err.into()))?;
        } else {
            error!(
                target: EVENT_TARGET,
                ?drep,
                "remove.unknown_drep",
            )
        }
    }

    Ok(())
}
