use crate::in_memory::MemoryStore;
use amaru_ledger::store::{
    columns::cc_members::{Key, Row, Value},
    StoreError,
};

pub fn add(
    store: &MemoryStore,
    rows: impl Iterator<Item = (Key, Value)>,
) -> Result<(), StoreError> {
    let mut cc_members = store.cc_members.borrow_mut();

    for (key, hot_credential) in rows {
        let mut row = cc_members.get(&key).cloned().unwrap_or(Row {
            hot_credential: None,
        });

        hot_credential.set_or_reset(&mut row.hot_credential);
        cc_members.insert(key, row);
    }

    Ok(())
}

pub fn remove(store: &MemoryStore, rows: impl Iterator<Item = Key>) -> Result<(), StoreError> {
    let mut cc_members = store.cc_members.borrow_mut();

    for key in rows {
        cc_members.remove(&key);
    }

    Ok(())
}
