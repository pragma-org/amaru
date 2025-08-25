// Copyright 2025 PRAGMA
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

use crate::in_memory::MemoryStore;
use amaru_kernel::{CertificatePointer, DRepRegistration};
use amaru_ledger::store::{
    StoreError,
    columns::dreps::{Key, Row, Value},
};
use tracing::error;

pub fn add(
    store: &MemoryStore,
    rows: impl Iterator<Item = (Key, Value)>,
) -> Result<(), StoreError> {
    let mut dreps = store.dreps.borrow_mut();

    for (credential, (anchor, registration)) in rows {
        if let Some(row) = dreps.get_mut(&credential) {
            // Re-registration or update
            if let Some(DRepRegistration {
                deposit,
                registered_at,
                valid_until,
                ..
            }) = registration
            {
                row.deposit = deposit;
                row.registered_at = registered_at;
                row.valid_until = valid_until;
            }

            anchor.set_or_reset(&mut row.anchor);
        } else if let Some(DRepRegistration {
            deposit,
            registered_at,
            valid_until,
            ..
        }) = registration
        {
            // New registration
            let mut row = Row {
                deposit,
                registered_at,
                valid_until,
                anchor: None,
                previous_deregistration: None,
            };
            anchor.set_or_reset(&mut row.anchor);
            dreps.insert(credential, row);
        } else {
            error!(
                target: "store::dreps::add",
                ?credential,
                "add.register_no_deposit",
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
