use crate::in_memory::MemoryStore;
use amaru_kernel::TransactionInput;
use amaru_ledger::store::{
    columns::utxo::{Key, Value},
    StoreError,
};

pub fn add(
    store: &MemoryStore,
    rows: impl Iterator<Item = (Key, Value)>,
) -> Result<(), StoreError> {
    let mut utxos = store.utxos.borrow_mut();

    for (input, output) in rows {
        utxos.insert(input, output);
    }

    Ok(())
}

pub fn remove(
    store: &MemoryStore,
    rows: impl Iterator<Item = TransactionInput>,
) -> Result<(), StoreError> {
    let mut utxos = store.utxos.borrow_mut();

    for input in rows {
        utxos.remove(&input);
    }

    Ok(())
}
