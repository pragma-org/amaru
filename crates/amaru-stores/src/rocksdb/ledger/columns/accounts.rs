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
    AsHash, CertificatePointer, DRep, Lovelace, PROTOCOL_VERSION_9, ProtocolVersion,
    StakeCredential, StakeCredentialKind,
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
use rocksdb::{DBPinnableSlice, Transaction};
use tracing::{debug, error};

/// Name prefixed used for storing Account entries. UTF-8 encoding for "acct"
pub const PREFIX: [u8; PREFIX_LEN] = [0x61, 0x63, 0x63, 0x74];

/// A special handler to reproduce the PROTOCOL_VERSION_9 bug that is (wrongly) removing
/// delegations of past delegators when unregistering a drep. However, we pass in the the
/// certificate pointer of the drep de-registration to check for cases where unregistration and
/// re-delegation happen within the same block. When they happen in the same transaction, the
/// delegation always take precedence over the unregistration.
///
/// This function is only needed for PROTOCOL_VERSION_9.
pub fn reset_delegation<DB>(
    db: &Transaction<'_, DB>,
    rows: impl Iterator<Item = (Key, CertificatePointer)>,
) -> Result<(), StoreError> {
    for (credential, unregistered_at) in rows {
        let key = as_key(&PREFIX, &credential);

        let entry = db
            .get_pinned(&key)
            .map_err(|err| StoreError::Internal(err.into()))?
            .map(|d| unsafe_decode::<Row>(&d));

        if let Some(mut row) = entry {
            // Check whether the existing delegation is not taking precedence over the previous
            // drep de-registration. We require this because we do not clean up accounts's drep
            // field when drep unregisters (to avoid traversing all accounts rows on each drep
            // de-registration).
            //
            // Instead, we record the certificate since when a delegation was established, as
            // well as the certificates from when a DRep retired.
            //
            // In the case where our previous DRep has *already retired*, then we must NOT retain
            // clean up the past delegation (it is virtually already gone). This can happen when
            // retirement and re-delegation happens in the same block (or even, same transaction).
            //
            // It's worth considering a few example to wrap one's head around:
            //
            // ===== 1
            //
            // - Account delegates to Alice.
            // - Account delegates to Bob.
            // - Alice retires.
            //
            // => Account is now *undelegated*.
            //
            // ===== 2
            //
            // - Account delegates to Alice.
            // - Alice retires.
            // - Account delegates to Bob.
            //
            // => Account is still delegated to *Bob*.
            //
            // ===== 3
            //
            // - Account delegates to Alice.
            // - Alice retires.
            // - Account delegates to Bob.
            // - Alice re-registers.
            // - Alice retires.
            //
            // => Account is still delegated to *Bob*.
            if let Some((_, delegated_since)) = row.drep
                && delegated_since > unregistered_at
            {
                debug!(
                    unregistered.at = %unregistered_at,
                    redelegated.since = %delegated_since,
                    delegator.type = %StakeCredentialKind::from(&credential),
                    delegator.hash = %credential.as_hash(),
                    "delegator has already re-delegated; ignoring previous drep de-registration",
                );
            } else {
                row.drep = None;
                db.put(key, as_value(row))
                    .map_err(|err| StoreError::Internal(err.into()))?;
            }
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

        let new_drep_is_predefined = matches!(
            drep,
            Resettable::Set((DRep::Abstain, _)) | Resettable::Set((DRep::NoConfidence, _))
        );

        // In case where a registration already exists, then we must only update the underlying
        // entry, while preserving the reward amount.
        let previous_drep = if let Some(mut row) = db
            .get_pinned(&key)
            .map_err(|err| StoreError::Internal(err.into()))?
            .map(|d| unsafe_decode::<Row>(&d))
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

        // NOTE(PROTOCOL_VERSION_9):
        // In protocol version 9, a subtle bug causes registered DRep to retain their past
        // delegators (then causing a cascade of downstream inconsistencies).
        //
        // So, we do keep track of past delegations in v9, but only when the new DRep isn't a
        // pre-defined DRep. In this particular case, the bug doesn't apply. Joy.
        if protocol_version <= PROTOCOL_VERSION_9
            && let Some((previous_drep, previous_since)) = previous_drep
        {
            if new_drep_is_predefined {
                dreps_delegations::remove(db, &previous_drep, &credential)?;
            } else {
                previous_delegations.push((credential, previous_drep, previous_since));
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
            .get_pinned(&key)
            .map_err(|err| StoreError::Internal(err.into()))?
            .map(|d| unsafe_decode::<Row>(&d))
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
pub fn get<'a>(
    db_get: impl Fn(&[u8]) -> Result<Option<DBPinnableSlice<'a>>, rocksdb::Error>,
    credential: &Key,
) -> Result<Option<Row>, StoreError> {
    let key = as_key(&PREFIX, credential);
    let bytes = db_get(&key);
    bytes
        .map_err(|err| StoreError::Internal(err.into()))
        .map(|opt| opt.map(|d| unsafe_decode::<Row>(&d)))
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
        .get_pinned(&key)
        .map_err(|err| StoreError::Internal(err.into()))?
        .map(|d| unsafe_decode::<Row>(&d))
    {
        row.rewards = with_rewards(row.rewards);
        db.put(key, as_value(row))
            .map_err(|err| StoreError::Internal(err.into()))?;
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
