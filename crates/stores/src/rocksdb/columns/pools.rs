use super::super::common::{as_key, as_value, PREFIX_LEN};
use crate::rocksdb::scolumns::pools::{Key, Row, Value, EVENT_TARGET};
use amaru_ledger::kernel::{Epoch, PoolId};
use rocksdb::{self, OptimisticTransactionDB, ThreadMode, Transaction};
use tracing::error;

/// Name prefixed used for storing Pool entries. UTF-8 encoding for "pool"
pub const PREFIX: [u8; PREFIX_LEN] = [0x70, 0x6f, 0x6f, 0x6c];

pub fn get<T: ThreadMode>(
    db: &OptimisticTransactionDB<T>,
    pool: &PoolId,
) -> Result<Option<Row>, rocksdb::Error> {
    Ok(db.get(as_key(&PREFIX, pool))?.map(Row::unsafe_decode))
}

pub fn add<DB>(
    db: &Transaction<'_, DB>,
    rows: impl Iterator<Item = Value>,
) -> Result<(), rocksdb::Error> {
    for (params, epoch) in rows {
        let pool = params.id;

        // Pool parameters are stored in an epoch-aware fashion.
        //
        // - If no parameters exist for the pool, we can immediately create a new
        //   entry.
        //
        // - If one already exists, then the parameters are stashed until the next
        //   epoch boundary.
        //
        // TODO: We might want to define a MERGE OPERATOR to speed this up if
        // necessary.
        let params = match db.get(as_key(&PREFIX, pool))? {
            None => as_value(Row::new(params)),
            Some(existing_params) => Row::extend(existing_params, (Some(params), epoch)),
        };

        db.put(as_key(&PREFIX, pool), params)?;
    }

    Ok(())
}

pub fn remove<DB>(
    db: &Transaction<'_, DB>,
    rows: impl Iterator<Item = (Key, Epoch)>,
) -> Result<(), rocksdb::Error> {
    for (pool, epoch) in rows {
        // We do not delete pool immediately but rather schedule the
        // removal as an empty parameter update. The 'pool reaping' happens on
        // every epoch boundary.
        match db.get(as_key(&PREFIX, pool))? {
            None => {
                error!(target: EVENT_TARGET, ?pool, "remove.unknown")
            }
            Some(existing_params) => db.put(
                as_key(&PREFIX, pool),
                Row::extend(existing_params, (None, epoch)),
            )?,
        };
    }

    Ok(())
}
