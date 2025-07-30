use crate::in_memory::MemoryStore;
use amaru_kernel::ComparableProposalId;
use amaru_ledger::store::{
    columns::proposals::{Key, Value},
    StoreError,
};

pub fn add(
    store: &MemoryStore,
    rows: impl Iterator<Item = (Key, Value)>,
) -> Result<(), StoreError> {
    for (key, value) in rows {
        store
            .proposals
            .borrow_mut()
            .insert(ComparableProposalId::from(key), value);
    }

    Ok(())
}

pub fn remove(store: &MemoryStore, rows: impl Iterator<Item = Key>) -> Result<(), StoreError> {
    let mut proposals = store.proposals.borrow_mut();

    for key in rows {
        proposals.remove(&ComparableProposalId::from(key));
    }

    Ok(())
}
