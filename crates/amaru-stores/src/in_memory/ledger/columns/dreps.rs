use crate::in_memory::MemoryStore;
use amaru_kernel::{CertificatePointer, Point, StakeCredential};
use amaru_ledger::store::{
    columns::dreps::{Key, Row, Value},
    StoreError,
};
use std::collections::BTreeSet;
use tracing::error;

pub fn add(
    store: &MemoryStore,
    rows: impl Iterator<Item = (Key, Value)>,
) -> Result<(), StoreError> {
    let mut dreps = store.dreps.borrow_mut();

    for (credential, (anchor, register, epoch)) in rows {
        let row = if let Some(mut row) = dreps.get(&credential).cloned() {
            // Re-registration or update
            if let Some((deposit, registered_at)) = register {
                row.registered_at = registered_at;
                row.deposit = deposit;
                row.last_interaction = None;
            } else {
                row.last_interaction = Some(epoch);
            }

            anchor.set_or_reset(&mut row.anchor);
            row
        } else if let Some((deposit, registered_at)) = register {
            // New registration
            let mut row = Row {
                anchor: None,
                deposit,
                registered_at,
                last_interaction: None,
                previous_deregistration: None,
            };
            anchor.set_or_reset(&mut row.anchor);
            row
        } else {
            error!(
                target: "store::dreps::add",
                ?credential,
                "add.register_no_deposit",
            );
            continue;
        };

        dreps.insert(credential, row);
    }

    Ok(())
}

pub fn tick(
    store: &MemoryStore,
    voting_dreps: &BTreeSet<StakeCredential>,
    point: &Point,
) -> Result<(), StoreError> {
    let epoch = store
        .era_history
        .slot_to_epoch(point.slot_or_default(), point.slot_or_default())
        .map_err(|err| StoreError::Internal(err.into()))?;

    let mut dreps = store.dreps.borrow_mut();

    for credential in voting_dreps {
        if let Some(row) = dreps.get_mut(credential) {
            row.last_interaction = Some(epoch);
        } else {
            tracing::error!(
                target: "store::dreps::tick",
                ?credential,
                "tick.unknown_drep",
            );
        }
    }

    Ok(())
}

pub fn remove(
    store: &MemoryStore,
    rows: impl Iterator<Item = (Key, CertificatePointer)>,
) -> Result<(), StoreError> {
    let mut dreps = store.dreps.borrow_mut();

    for (credential, pointer) in rows {
        if let Some(row) = dreps.get_mut(&credential) {
            row.previous_deregistration = Some(pointer);
        } else {
            error!(
                target: "store::dreps::remove",
                ?credential,
                "remove.unknown_drep",
            );
        }
    }

    Ok(())
}
