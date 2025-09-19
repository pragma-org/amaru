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

use crate::rocksdb::{
    accounts,
    common::{PREFIX_LEN, as_key, as_value},
    dreps_delegations,
};
use amaru_kernel::{
    CertificatePointer, DRepRegistration, Epoch, PROTOCOL_VERSION_9, ProtocolVersion,
    StakeCredential,
};
use amaru_ledger::store::{
    StoreError,
    columns::{
        dreps::{EVENT_TARGET, Key, Row, Value},
        unsafe_decode,
    },
};
use rocksdb::Transaction;
use std::collections::BTreeSet;
use tracing::{error, warn};

/// Name prefixed used for storing DReps entries. UTF-8 encoding for "drep"
pub const PREFIX: [u8; PREFIX_LEN] = [0x64, 0x72, 0x65, 0x70];

/// Register a new DRep.
pub fn add<DB>(
    db: &Transaction<'_, DB>,
    valid_until_on_update: Epoch,
    rows: impl Iterator<Item = (Key, Value)>,
) -> Result<(), StoreError> {
    for (credential, (anchor, registration)) in rows {
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
            .map(unsafe_decode::<Row>)
        {
            // Re-registration
            if let Some(DRepRegistration {
                deposit,
                registered_at,
                valid_until,
                ..
            }) = registration
            {
                row.deposit = deposit;
                row.registered_at = registered_at;
                row.valid_until = valid_until;
            } else {
                row.valid_until = valid_until_on_update;
            }

            Some(row)
        } else if let Some(DRepRegistration {
            deposit,
            registered_at,
            valid_until,
            ..
        }) = registration
        {
            // Brand new registration.
            Some(Row {
                deposit,
                registered_at,
                valid_until,
                anchor: None,
                previous_deregistration: None,
            })
        } else {
            // Technically impossible, sign of a logic error.
            None
        };

        match row {
            Some(mut row) => {
                anchor.set_or_reset(&mut row.anchor);

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

/// Re-calculate drep expiry based the current epoch. This happens each time a drep vote on an
/// active governance proposal.
pub fn set_valid_until<DB>(
    db: &Transaction<'_, DB>,
    credentials: BTreeSet<StakeCredential>,
    valid_until: Epoch,
) -> Result<(), StoreError> {
    for credential in credentials {
        let key = as_key(&PREFIX, &credential);

        if let Some(mut row) = db
            .get(&key)
            .map_err(|err| StoreError::Internal(err.into()))?
            .map(unsafe_decode::<Row>)
        {
            row.valid_until = valid_until;
            db.put(key, as_value(row))
                .map_err(|err| StoreError::Internal(err.into()))?;
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
pub fn remove<DB>(
    db: &Transaction<'_, DB>,
    rows: impl Iterator<Item = (Key, CertificatePointer)>,
    protocol_version: ProtocolVersion,
) -> Result<(), StoreError> {
    for (drep, pointer) in rows {
        let key = as_key(&PREFIX, &drep);

        // NOTE: Due to a bug in protocol version 9, we need to clear any delegation relation that
        // *ever* existed between this DRep and its delegators. That is the case even if the
        // delegators are no longer delegated to the drep, but were at some point in the past.
        //
        // The `dreps_delegators` column remembers exactly this information. When we clean it from
        // the DRep being removed, it yields back all the accounts that have been delegated to the
        // DRep during its lifetime. And we unbind all of them.
        if protocol_version <= PROTOCOL_VERSION_9 {
            let resets = dreps_delegations::drop(db, &drep)?
                .into_iter()
                .map(|delegator| (delegator, pointer));
            accounts::reset_delegation(db, resets)?;
        }

        if let Some(mut row) = db
            .get(&key)
            .map_err(|err| StoreError::Internal(err.into()))?
            .map(unsafe_decode::<Row>)
        {
            row.previous_deregistration = Some(pointer);
            db.put(key, as_value(row))
                .map_err(|err| StoreError::Internal(err.into()))?;
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
