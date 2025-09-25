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

use crate::rocksdb::{
    common::{PREFIX_LEN, as_key, as_value},
    dreps_delegations,
};
use amaru_kernel::{
    CertificatePointer, DRep, Lovelace, PROTOCOL_VERSION_9, ProtocolVersion, StakeCredential,
    StakeCredentialType, stake_credential_hash,
};
use amaru_ledger::{
    state::diff_bind::Resettable,
    store::{
        StoreError,
        columns::{
            accounts::{EVENT_TARGET, Key, Row, Value},
            unsafe_decode,
        },
    },
};
use rocksdb::Transaction;
use tracing::{debug, error};

/// Name prefixed used for storing Account entries. UTF-8 encoding for "acct"
pub const PREFIX: [u8; PREFIX_LEN] = [0x61, 0x63, 0x63, 0x74];

/// A special handler to reproduce the v9 bug that is (wrongly) removing delegations of past
/// delegators when unregistering a drep. However, we pass in the the certificate pointer of the
/// drep de-registration to check for cases where unregistration and re-delegation happen within
/// the same block. When they happen in the same transaction, the delegation always take precedence
/// over the unregistration.
pub fn reset_delegation<DB>(
    db: &Transaction<'_, DB>,
    rows: impl Iterator<Item = (Key, CertificatePointer)>,
) -> Result<(), StoreError> {
    for (credential, _unregistered_at) in rows {
        let key = as_key(&PREFIX, &credential);

        let entry = db
            .get(&key)
            .map_err(|err| StoreError::Internal(err.into()))?
            .map(unsafe_decode::<Row>);

        if let Some(mut row) = entry {
            row.drep = None;
            db.put(key, as_value(row))
                .map_err(|err| StoreError::Internal(err.into()))?;
        }
    }

    Ok(())
}

/// Register a new credential, with or without a stake pool.
pub fn add<DB>(
    db: &Transaction<'_, DB>,
    rows: impl Iterator<Item = (Key, Value)>,
    protocol_version: ProtocolVersion,
) -> Result<Vec<(StakeCredential, DRep, CertificatePointer)>, StoreError> {
    let mut previous_delegations = Vec::new();

    for (credential, (pool, drep, deposit, rewards)) in rows {
        let key = as_key(&PREFIX, &credential);

        let is_predefined_drep = matches!(
            drep,
            Resettable::Set((DRep::Abstain, _)) | Resettable::Set((DRep::NoConfidence, _))
        );

        // In case where a registration already exists, then we must only update the underlying
        // entry, while preserving the reward amount.
        let previous_drep = if let Some(mut row) = db
            .get(&key)
            .map_err(|err| StoreError::Internal(err.into()))?
            .map(unsafe_decode::<Row>)
        {
            pool.set_or_reset(&mut row.pool);
            let previous_drep = drep.set_or_reset(&mut row.drep);

            if let Some(deposit) = deposit {
                row.deposit = deposit;
            }

            db.put(key, as_value(row))
                .map_err(|err| StoreError::Internal(err.into()))?;

            Ok::<_, StoreError>(previous_drep)
        } else if let Some(deposit) = deposit {
            let mut row = Row {
                deposit,
                pool: None,
                drep: None,
                rewards,
            };

            pool.set_or_reset(&mut row.pool);
            let previous_drep = drep.set_or_reset(&mut row.drep);

            db.put(key, as_value(row))
                .map_err(|err| StoreError::Internal(err.into()))?;

            Ok(previous_drep)
        } else {
            unreachable!(
                "attempted to create an account without a deposit: account={:?}",
                credential
            );
        }?;

        // NOTE:
        // In protocol version 9, a subtle bug causes registered DRep to retain their past
        // delegators (then causing a cascade of downstream inconsistencies).
        //
        // So, we do keep track of past delegations in v9, but only when the new DRep isn't a
        // pre-defined DRep. In this particular case, the bug doesn't apply. Joy.
        if protocol_version <= PROTOCOL_VERSION_9
            && let Some((previous_drep, since)) = previous_drep
        {
            if is_predefined_drep {
                dreps_delegations::remove(db, &previous_drep, &credential)?;
            } else {
                previous_delegations.push((credential, previous_drep, since));
            }
        }
    }

    Ok(previous_delegations)
}

/// Reset rewards counter of many accounts.
pub fn reset_many<DB>(
    db: &Transaction<'_, DB>,
    rows: impl Iterator<Item = Key>,
) -> Result<(), StoreError> {
    for credential in rows {
        let key = as_key(&PREFIX, &credential);

        if let Some(mut row) = db
            .get(&key)
            .map_err(|err| StoreError::Internal(err.into()))?
            .map(unsafe_decode::<Row>)
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

/// Obtain a account from the store
pub fn get(
    db_get: impl Fn(&[u8]) -> Result<Option<Vec<u8>>, rocksdb::Error>,
    credential: &Key,
) -> Result<Option<Row>, StoreError> {
    let key = as_key(&PREFIX, credential);
    let bytes = db_get(&key);
    bytes
        .map_err(|err| StoreError::Internal(err.into()))
        .map(|opt| opt.map(unsafe_decode::<Row>))
}

/// Alter balance of a specific account. If the account did not exist, returns the leftovers
/// amount that couldn't be allocated to the account.
pub fn set<DB>(
    db: &Transaction<'_, DB>,
    credential: &Key,
    with_rewards: impl FnOnce(Lovelace) -> Lovelace,
) -> Result<Lovelace, StoreError> {
    let key = as_key(&PREFIX, credential);

    if let Some(mut row) = db
        .get(&key)
        .map_err(|err| StoreError::Internal(err.into()))?
        .map(unsafe_decode::<Row>)
    {
        row.rewards = with_rewards(row.rewards);
        db.put(key, as_value(row))
            .map_err(|err| StoreError::Internal(err.into()))?;
        return Ok(0);
    }

    debug!(
        target: EVENT_TARGET,
        type = %StakeCredentialType::from(credential),
        account = %stake_credential_hash(credential),
        "set.no_account",
    );

    Ok(with_rewards(0))
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
