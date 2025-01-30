use super::super::common::{as_key, as_value, PREFIX_LEN};
use crate::rocksdb::scolumns::accounts::{Key, Row, Value, EVENT_TARGET};
use amaru_ledger::store::StoreError;
use rocksdb::Transaction;
use tracing::error;

/// Name prefixed used for storing Account entries. UTF-8 encoding for "acct"
pub const PREFIX: [u8; PREFIX_LEN] = [0x61, 0x63, 0x63, 0x74];

/// Register a new credential, with or without a stake pool.
pub fn add<DB>(
    db: &Transaction<'_, DB>,
    rows: impl Iterator<Item = (Key, Value)>,
) -> Result<(), StoreError> {
    for (credential, (delegatee, deposit, rewards)) in rows {
        let key = as_key(&PREFIX, &credential);

        // In case where a registration already exists, then we must only update the underlying
        // entry, while preserving the reward amount.
        if let Some(mut row) = db
            .get(&key)
            .map_err(|err| StoreError::Internal(err.into()))?
            .map(Row::unsafe_decode)
        {
            row.delegatee = delegatee;
            if let Some(deposit) = deposit {
                row.deposit = deposit;
            }
            db.put(key, as_value(row))
                .map_err(|err| StoreError::Internal(err.into()))?;
        } else if let Some(deposit) = deposit {
            let row = Row {
                delegatee,
                deposit,
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
