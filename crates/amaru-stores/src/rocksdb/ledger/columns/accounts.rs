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

use amaru_kernel::{AsHash, CredentialKind, Epoch, Lovelace};
use amaru_ledger::store::{
    StoreError,
    columns::{
        accounts::{Key, Row, Value},
        unsafe_decode,
    },
};
use amaru_observability::{debug, error, trace_span};
use rocksdb::{DBPinnableSlice, Transaction};

use crate::rocksdb::{
    common::{PREFIX_LEN, as_key, as_value},
    recently_unregistered_accounts,
};

/// Name prefixed used for storing Account entries. UTF-8 encoding for "acct"
pub const PREFIX: [u8; PREFIX_LEN] = [0x61, 0x63, 0x63, 0x74];

/// Register a new credential, with or without a stake pool.
pub fn add<DB>(db: &Transaction<'_, DB>, rows: impl Iterator<Item = (Key, Value)>) -> Result<(), StoreError> {
    trace_span!(stores::ledger::accounts::ADD).in_scope(|| {
        for (credential, value) in rows {
            let key = as_key(&PREFIX, credential);

            let existing =
                db.get_pinned(&key).map_err(|err| StoreError::Internal(err.into()))?.map(|d| unsafe_decode::<Row>(&d));

            let row = match (value, existing) {
                (Value::Create { pool, drep, deposit, rewards }, _) => {
                    let mut row = Row { deposit, pool: None, drep: None, rewards };
                    pool.set_or_reset(&mut row.pool);
                    drep.set_or_reset(&mut row.drep);

                    recently_unregistered_accounts::remove(db, &credential)?;

                    row
                }

                (Value::Update { pool, drep }, Some(mut row)) => {
                    pool.set_or_reset(&mut row.pool);
                    drep.set_or_reset(&mut row.drep);
                    row
                }

                (Value::Update { .. }, None) => {
                    unreachable!("attempted to update a non-existing account: account={:?}", credential)
                }
            };

            db.put(key, as_value(row)).map_err(|err| StoreError::Internal(err.into()))?;
        }

        Ok(())
    })
}

/// Reset rewards counter of many accounts.
pub fn reset_many<DB>(db: &Transaction<'_, DB>, rows: impl Iterator<Item = Key>) -> Result<(), StoreError> {
    trace_span!(stores::ledger::accounts::RESET_MANY).in_scope(|| {
        for credential in rows {
            let key = as_key(&PREFIX, credential);

            if let Some(mut row) =
                db.get_pinned(&key).map_err(|err| StoreError::Internal(err.into()))?.map(|d| unsafe_decode::<Row>(&d))
            {
                row.rewards = 0;
                db.put(key, as_value(row)).map_err(|err| StoreError::Internal(err.into()))?;
            } else {
                error!(stores::ledger::accounts::RESET_MANY, credential, reason = "no account for given credential");
            }
        }

        Ok(())
    })
}

/// Obtain a account from the store
pub fn get<'a>(
    db_get: impl Fn(&[u8]) -> Result<Option<DBPinnableSlice<'a>>, rocksdb::Error>,
    credential: &Key,
) -> Result<Option<Row>, StoreError> {
    trace_span!(stores::ledger::accounts::GET).in_scope(|| {
        let key = as_key(&PREFIX, credential);
        let bytes = db_get(&key);
        bytes.map_err(|err| StoreError::Internal(err.into())).map(|opt| opt.map(|d| unsafe_decode::<Row>(&d)))
    })
}

/// Alter balance of a specific account. If the account did not exist, returns the leftovers
/// amount that couldn't be allocated to the account.
pub fn set_rewards<DB>(
    db: &Transaction<'_, DB>,
    credential: &Key,
    with_rewards: impl FnOnce(Lovelace) -> Lovelace,
) -> Result<Lovelace, StoreError> {
    trace_span!(stores::ledger::accounts::SET).in_scope(|| {
        let key = as_key(&PREFIX, credential);

        if let Some(mut row) =
            db.get_pinned(&key).map_err(|err| StoreError::Internal(err.into()))?.map(|d| unsafe_decode::<Row>(&d))
        {
            row.rewards = with_rewards(row.rewards);
            db.put(key, as_value(row)).map_err(|err| StoreError::Internal(err.into()))?;
            return Ok(0);
        }

        // TODO: Should probably be an error now that we have the overlay...
        debug!(
            stores::ledger::accounts::SET,
            credential_type = CredentialKind::from(credential),
            account = credential.as_hash(),
            reason = "cannot set stake, account is gone"
        );

        Ok(with_rewards(0))
    })
}

/// Clear a stake credential registration.
pub fn remove<DB>(db: &Transaction<'_, DB>, rows: impl Iterator<Item = Key>, epoch: Epoch) -> Result<(), StoreError> {
    trace_span!(stores::ledger::accounts::REMOVE).in_scope(|| {
        for credential in rows {
            recently_unregistered_accounts::insert(db, &credential, epoch)?;
            db.delete(as_key(&PREFIX, credential)).map_err(|err| StoreError::Internal(err.into()))?;
        }

        Ok(())
    })
}
