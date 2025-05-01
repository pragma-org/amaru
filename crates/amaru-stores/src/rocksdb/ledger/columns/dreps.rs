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

use crate::rocksdb::common::{as_key, as_value, PREFIX_LEN};
use amaru_kernel::{CertificatePointer, Epoch, StakeCredential};
use amaru_ledger::store::{
    columns::dreps::{Key, Row, Value, EVENT_TARGET},
    StoreError,
};
use rocksdb::Transaction;
use std::collections::BTreeSet;
use tracing::error;

/// Name prefixed used for storing DReps entries. UTF-8 encoding for "drep"
pub const PREFIX: [u8; PREFIX_LEN] = [0x64, 0x72, 0x65, 0x70];

/// Register a new DRep.
#[allow(clippy::unwrap_used)]
pub fn add<DB>(
    db: &Transaction<'_, DB>,
    rows: impl Iterator<Item = (Key, Value)>,
) -> Result<(), StoreError> {
    for (credential, (anchor, register, epoch)) in rows {
        let key = as_key(&PREFIX, &credential);

        // Registration already exists. Which can represents one of two cases:
        //
        // 1. The DRep is simply updating (register is None).
        // 2. The DRep has unregistered and is now re-registering.
        //
        // The latter is possible since we do not delete DRep from storage when they unregister;
        // but instead, we record the de-registration event; necessary to reconstruct a "valid"
        // ledger state down the line.
        let row = if let Some(mut row) = db
            .get(&key)
            .map_err(|err| StoreError::Internal(err.into()))?
            .map(Row::unsafe_decode)
        {
            // Re-registration
            if let Some((deposit, registered_at)) = register {
                row.registered_at = registered_at;
                row.deposit = deposit;
                row.last_interaction = None;
            } else {
                row.last_interaction = Some(epoch);
            }
            anchor.set_or_reset(&mut row.anchor);
            Some(row)

        // Brand new registration.
        } else if let Some((deposit, registered_at)) = register {
            let mut row = Row {
                anchor: None,
                deposit,
                registered_at,
                last_interaction: None,
                previous_deregistration: None,
            };
            anchor.set_or_reset(&mut row.anchor);
            Some(row)

        // Technically impossible, sign of a logic error.
        } else {
            None
        };

        match row {
            Some(row) => {
                db.put(key, as_value(row))
                    .map_err(|err| StoreError::Internal(err.into()))?;
            }
            None => {
                error!(
                    target: EVENT_TARGET,
                    ?credential,
                    "add.register_no_deposit",
                )
            }
        }
    }

    Ok(())
}

pub fn tick<DB>(
    db: &Transaction<'_, DB>,
    credentials: BTreeSet<StakeCredential>,
    epoch: Epoch,
) -> Result<(), StoreError> {
    for credential in credentials {
        let key = as_key(&PREFIX, &credential);

        // In case where a registration already exists, then we must only update the underlying
        // entry.
        if let Some(mut row) = db
            .get(&key)
            .map_err(|err| StoreError::Internal(err.into()))?
            .map(Row::unsafe_decode)
        {
            row.last_interaction = Some(epoch);
            db.put(key, as_value(row))
                .map_err(|err| StoreError::Internal(err.into()))?;
        } else {
            error!(
                target: EVENT_TARGET,
                ?credential,
                "tick.unknown_drep",
            )
        };
    }

    Ok(())
}

/// Clear a DRep registration.
pub fn remove<DB>(
    db: &Transaction<'_, DB>,
    rows: impl Iterator<Item = (Key, CertificatePointer)>,
) -> Result<(), StoreError> {
    for (credential, pointer) in rows {
        let key = as_key(&PREFIX, &credential);

        if let Some(mut row) = db
            .get(&key)
            .map_err(|err| StoreError::Internal(err.into()))?
            .map(Row::unsafe_decode)
        {
            row.previous_deregistration = Some(pointer);
            db.put(key, as_value(row))
                .map_err(|err| StoreError::Internal(err.into()))?;
        } else {
            error!(
                target: EVENT_TARGET,
                ?credential,
                "remove.unknown_drep",
            )
        }
    }

    Ok(())
}
