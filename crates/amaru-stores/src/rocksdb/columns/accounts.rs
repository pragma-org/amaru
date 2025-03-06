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

use crate::rocksdb::common::{as_key, as_value, PREFIX_LEN};
use amaru_ledger::store::StoreError;
use rocksdb::Transaction;
use tracing::error;

use amaru_ledger::store::columns::accounts::{Key, Row, Value, EVENT_TARGET};

/// Name prefixed used for storing Account entries. UTF-8 encoding for "acct"
pub const PREFIX: [u8; PREFIX_LEN] = [0x61, 0x63, 0x63, 0x74];

/// Register a new credential, with or without a stake pool.
pub fn add<DB>(
    db: &Transaction<'_, DB>,
    rows: impl Iterator<Item = (Key, Value)>,
) -> Result<(), StoreError> {
    for (credential, (delegatee, drep, deposit, rewards)) in rows {
        let key = as_key(&PREFIX, &credential);

        // In case where a registration already exists, then we must only update the underlying
        // entry, while preserving the reward amount.
        if let Some(mut row) = db
            .get(&key)
            .map_err(|err| StoreError::Internal(err.into()))?
            .map(Row::unsafe_decode)
        {
            // NOTE: One cannot remove a delegatee or a drep. So if an update contains a `None`
            // delegatee or drep, then there's actually nothing to do. Crucially, we shall not
            // erase either of the drep or the delegatee when binding the other for the first
            // time.
            row.delegatee = delegatee.or(row.delegatee);
            row.drep = drep.or(row.drep);

            if let Some(deposit) = deposit {
                row.deposit = deposit;
            }
            db.put(key, as_value(row))
                .map_err(|err| StoreError::Internal(err.into()))?;
        } else if let Some(deposit) = deposit {
            let row = Row {
                delegatee,
                deposit,
                drep,
                rewards,
            };
            db.put(key, as_value(row))
                .map_err(|err| StoreError::Internal(err.into()))?;
        } else {
            error!(
                target: EVENT_TARGET,
                ?credential,
                "add.register_no_deposit",
            )
        };
    }

    Ok(())
}

/// Reset rewards counter of many accounts.
pub fn reset<DB>(
    db: &Transaction<'_, DB>,
    rows: impl Iterator<Item = Key>,
) -> Result<(), StoreError> {
    for credential in rows {
        let key = as_key(&PREFIX, &credential);

        if let Some(mut row) = db
            .get(&key)
            .map_err(|err| StoreError::Internal(err.into()))?
            .map(Row::unsafe_decode)
        {
            row.rewards = 0;
            db.put(key, as_value(row))
                .map_err(|err| StoreError::Internal(err.into()))?;
        } else {
            error!(
                target: EVENT_TARGET,
                ?credential,
                "reset.no_account",
            )
        }
    }

    Ok(())
}

/// Clear a stake credential registration.
pub fn remove<DB>(
    db: &Transaction<'_, DB>,
    rows: impl Iterator<Item = Key>,
) -> Result<(), StoreError> {
    for credential in rows {
        db.delete(as_key(&PREFIX, &credential))
            .map_err(|err| StoreError::Internal(err.into()))?;
    }

    Ok(())
}
