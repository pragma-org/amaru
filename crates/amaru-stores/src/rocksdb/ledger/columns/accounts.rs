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

use amaru_kernel::{AsHash, Lovelace, StakeCredentialKind};
use amaru_ledger::store::{
    StoreError,
    columns::{
        accounts::{EVENT_TARGET, Key, Row, Value},
        unsafe_decode,
    },
};
use amaru_observability::debug_span;
use rocksdb::{DBPinnableSlice, Transaction};
use tracing::{debug, error};

use crate::rocksdb::common::{PREFIX_LEN, as_key, as_value};

/// Name prefixed used for storing Account entries. UTF-8 encoding for "acct"
pub const PREFIX: [u8; PREFIX_LEN] = [0x61, 0x63, 0x63, 0x74];

/// Register a new credential, with or without a stake pool.
pub fn add<DB>(db: &Transaction<'_, DB>, rows: impl Iterator<Item = (Key, Value)>) -> Result<(), StoreError> {
    let _span = debug_span!(
        stores::ledger::columns::ACCOUNTS_ADD,
        db_system_name = "rocksdb".to_string(),
        db_operation_name = "write".to_string(),
        db_collection_name = "account".to_string()
    );
    let _guard = _span.enter();

    for (credential, (pool, drep, deposit, rewards)) in rows {
        let key = as_key(&PREFIX, &credential);

        // In case where a registration already exists, then we must only update the underlying
        // entry, while preserving the reward amount.
        if let Some(mut row) =
            db.get_pinned(&key).map_err(|err| StoreError::Internal(err.into()))?.map(|d| unsafe_decode::<Row>(&d))
        {
            pool.set_or_reset(&mut row.pool);
            drep.set_or_reset(&mut row.drep);

            if let Some(deposit) = deposit {
                row.deposit = deposit;
            }

            db.put(key, as_value(row)).map_err(|err| StoreError::Internal(err.into()))?;
        } else if let Some(deposit) = deposit {
            let mut row = Row { deposit, pool: None, drep: None, rewards };

            pool.set_or_reset(&mut row.pool);
            drep.set_or_reset(&mut row.drep);

            db.put(key, as_value(row)).map_err(|err| StoreError::Internal(err.into()))?;
        } else {
            unreachable!("attempted to create an account without a deposit: account={:?}", credential);
        }
    }

    Ok(())
}

/// Reset rewards counter of many accounts.
pub fn reset_many<DB>(db: &Transaction<'_, DB>, rows: impl Iterator<Item = Key>) -> Result<(), StoreError> {
    let _span = debug_span!(
        stores::ledger::columns::ACCOUNTS_RESET_MANY,
        db_system_name = "rocksdb".to_string(),
        db_operation_name = "write".to_string(),
        db_collection_name = "account".to_string()
    );
    let _guard = _span.enter();

    for credential in rows {
        let key = as_key(&PREFIX, &credential);

        if let Some(mut row) =
            db.get_pinned(&key).map_err(|err| StoreError::Internal(err.into()))?.map(|d| unsafe_decode::<Row>(&d))
        {
            row.rewards = 0;
            db.put(key, as_value(row)).map_err(|err| StoreError::Internal(err.into()))?;
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

/// Obtain a account from the store
pub fn get<'a>(
    db_get: impl Fn(&[u8]) -> Result<Option<DBPinnableSlice<'a>>, rocksdb::Error>,
    credential: &Key,
) -> Result<Option<Row>, StoreError> {
    let _span = debug_span!(
        stores::ledger::columns::ACCOUNTS_GET,
        db_system_name = "rocksdb".to_string(),
        db_operation_name = "get".to_string(),
        db_collection_name = "account".to_string()
    );
    let _guard = _span.enter();

    let key = as_key(&PREFIX, credential);
    let bytes = db_get(&key);
    bytes.map_err(|err| StoreError::Internal(err.into())).map(|opt| opt.map(|d| unsafe_decode::<Row>(&d)))
}

/// Alter balance of a specific account. If the account did not exist, returns the leftovers
/// amount that couldn't be allocated to the account.
pub fn set<DB>(
    db: &Transaction<'_, DB>,
    credential: &Key,
    with_rewards: impl FnOnce(Lovelace) -> Lovelace,
) -> Result<Lovelace, StoreError> {
    let _span = debug_span!(
        stores::ledger::columns::ACCOUNTS_SET,
        db_system_name = "rocksdb".to_string(),
        db_operation_name = "write".to_string(),
        db_collection_name = "account".to_string()
    );
    let _guard = _span.enter();

    let key = as_key(&PREFIX, credential);

    if let Some(mut row) =
        db.get_pinned(&key).map_err(|err| StoreError::Internal(err.into()))?.map(|d| unsafe_decode::<Row>(&d))
    {
        row.rewards = with_rewards(row.rewards);
        db.put(key, as_value(row)).map_err(|err| StoreError::Internal(err.into()))?;
        return Ok(0);
    }

    debug!(
        target: EVENT_TARGET,
        type = %StakeCredentialKind::from(credential),
        account = %credential.as_hash(),
        "set.no_account",
    );

    Ok(with_rewards(0))
}

/// Clear a stake credential registration.
pub fn remove<DB>(db: &Transaction<'_, DB>, rows: impl Iterator<Item = Key>) -> Result<(), StoreError> {
    let _span = debug_span!(
        stores::ledger::columns::ACCOUNTS_REMOVE,
        db_system_name = "rocksdb".to_string(),
        db_operation_name = "delete".to_string(),
        db_collection_name = "account".to_string()
    );
    let _guard = _span.enter();

    for credential in rows {
        db.delete(as_key(&PREFIX, &credential)).map_err(|err| StoreError::Internal(err.into()))?;
    }

    Ok(())
}
