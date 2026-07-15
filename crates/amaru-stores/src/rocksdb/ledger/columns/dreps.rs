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

use amaru_kernel::{CertificatePointer, DRepRegistration, Epoch, StakeCredential};
use amaru_ledger::store::{
    StoreError,
    columns::{
        dreps::{Key, Row, Value},
        unsafe_decode,
    },
};
use amaru_observability::{error, trace_span, warn};
use rocksdb::{DBPinnableSlice, Transaction};

use crate::rocksdb::common::{PREFIX_LEN, as_key, as_value};

/// Name prefixed used for storing DReps entries. UTF-8 encoding for "drep"
pub const PREFIX: [u8; PREFIX_LEN] = [0x64, 0x72, 0x65, 0x70];

/// Retrieve a single DRep
pub fn get<'a>(
    db_get: impl Fn(&[u8]) -> Result<Option<DBPinnableSlice<'a>>, rocksdb::Error>,
    credential: &Key,
) -> Result<Option<Row>, StoreError> {
    let _span = trace_span!(
        stores::ledger::columns::DREPS_GET,
        db_system_name = "rocksdb".to_string(),
        db_operation_name = "get".to_string(),
        db_collection_name = "drep".to_string()
    );
    let _guard = _span.enter();

    let key = as_key(&PREFIX, credential);
    let bytes = db_get(&key);
    bytes.map_err(|err| StoreError::Internal(err.into())).map(|opt| opt.map(|d| unsafe_decode::<Row>(&d)))
}

/// Register a new DRep.
pub fn add<DB>(
    db: &Transaction<'_, DB>,
    valid_until_on_update: Epoch,
    rows: impl Iterator<Item = (Key, Value)>,
) -> Result<(), StoreError> {
    let _span = trace_span!(
        stores::ledger::columns::DREPS_ADD,
        db_system_name = "rocksdb".to_string(),
        db_operation_name = "write".to_string(),
        db_collection_name = "drep".to_string()
    );
    let _guard = _span.enter();

    for (credential, (anchor, registration)) in rows {
        let key = as_key(&PREFIX, &credential);

        // Registration already exists. Which represents one of two cases:
        //
        // 1. The DRep is simply updating (register is None).
        // 2. The DRep is re-registering after a previous deregistration.
        let row = if let Some(mut row) =
            db.get_pinned(&key).map_err(|err| StoreError::Internal(err.into()))?.map(|d| unsafe_decode::<Row>(&d))
        {
            // Re-registration
            if let Some(DRepRegistration { deposit, registered_at, valid_until, .. }) = registration {
                row.deposit = deposit;
                row.registered_at = registered_at;
                row.valid_until = valid_until;
            } else {
                row.valid_until = valid_until_on_update;
            }

            Some(row)
        } else if let Some(DRepRegistration { deposit, registered_at, valid_until, .. }) = registration {
            // Brand new registration.
            Some(Row { deposit, registered_at, valid_until, anchor: None })
        } else {
            // Technically impossible, sign of a logic error.
            None
        };

        match row {
            Some(mut row) => {
                anchor.set_or_reset(&mut row.anchor);

                db.put(key, as_value(row)).map_err(|err| StoreError::Internal(err.into()))?;
            }
            None => {
                error!(
                    target: "amaru::stores",
                    name: "dreps.add",
                    ?credential,
                    reason = "registration without a deposit"
                )
            }
        }
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
    trace_span!(
        stores::ledger::columns::DREPS_SET_VALID_UNTIL,
        db_system_name = "rocksdb".to_string(),
        db_operation_name = "write".to_string(),
        db_collection_name = "drep".to_string()
    )
    .in_scope(|| {
        for credential in credentials {
            let key = as_key(&PREFIX, &credential);

            if let Some(mut row) =
                db.get_pinned(&key).map_err(|err| StoreError::Internal(err.into()))?.map(|d| unsafe_decode::<Row>(&d))
            {
                row.valid_until = valid_until;
                db.put(key, as_value(row)).map_err(|err| StoreError::Internal(err.into()))?;
            } else {
                warn!(
                    target: "amaru::stores",
                    name: "dreps.set_valid_until",
                    ?credential,
                    reason = "unknown drep",
                )
            };
        }

        Ok(())
    })
}

/// Clear a DRep registration.
pub fn remove<DB>(
    db: &Transaction<'_, DB>,
    rows: impl Iterator<Item = (Key, CertificatePointer)>,
) -> Result<(), StoreError> {
    trace_span!(
        stores::ledger::columns::DREPS_REMOVE,
        db_system_name = "rocksdb".to_string(),
        db_operation_name = "write".to_string(),
        db_collection_name = "drep".to_string()
    )
    .in_scope(|| {
        for (drep, _) in rows {
            let key = as_key(&PREFIX, &drep);

            if db.get_pinned(&key).map_err(|err| StoreError::Internal(err.into()))?.is_some() {
                db.delete(key).map_err(|err| StoreError::Internal(err.into()))?;
            } else {
                error!(
                    target: "amaru::stores",
                    name: "dreps.remove",
                    ?drep,
                    reason = "unknown drep",
                )
            }
        }

        Ok(())
    })
}
