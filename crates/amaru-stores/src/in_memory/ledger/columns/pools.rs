use crate::in_memory::MemoryStore;
use amaru_ledger::store::{
    columns::pools::{Key, Row, Value},
    StoreError,
};
use slot_arithmetic::Epoch;
use tracing::error;

pub fn add(
    store: &MemoryStore,
    rows: impl Iterator<Item = Value>, // Value = (PoolParams, Epoch)
) {
    let mut pools = store.pools.borrow_mut();

    for (pool_params, epoch) in rows {
        let key = pool_params.id;

        let updated_row = match pools.get(&key).cloned() {
            Some(mut row) => {
                row.future_params.push((Some(pool_params.clone()), epoch));
                row
            }
            None => Row::new(pool_params.clone()),
        };

        pools.insert(key, updated_row);
    }
}

pub fn remove(
    store: &MemoryStore,
    rows: impl Iterator<Item = (Key, Epoch)>,
) -> Result<(), StoreError> {
    let mut pools = store.pools.borrow_mut();

    for (pool_id, epoch) in rows {
        match pools.get_mut(&pool_id) {
            Some(row) => {
                row.future_params.push((None, epoch));
            }
            None => {
                error!(target: "store::pools::remove", ?pool_id, "remove.unknown");
            }
        }
    }

    Ok(())
}
