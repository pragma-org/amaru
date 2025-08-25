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

use crate::rocksdb::common::{as_key, as_value, PREFIX_LEN};
use amaru_kernel::{stake_credential_hash, StakeCredentialType};
use amaru_ledger::{
    state::diff_bind::Resettable,
    store::{columns::unsafe_decode, StoreError},
};
use rocksdb::Transaction;
use tracing::error;

use amaru_ledger::store::columns::cc_members::{Key, Row, Value};

/// Name prefixed used for storing delegations entries. UTF-8 encoding for "comm"
pub const PREFIX: [u8; PREFIX_LEN] = [0x43, 0x4F, 0x4D, 0x4D];

/// Register a new CC Member.
pub fn add<DB>(
    db: &Transaction<'_, DB>,
    rows: impl Iterator<Item = (Key, Value)>,
) -> Result<(), StoreError> {
    for (cold_credential, (hot_credential, valid_until)) in rows {
        let key = as_key(&PREFIX, &cold_credential);

        let row = db
            .get(&key)
            .map_err(|err| StoreError::Internal(err.into()))?
            .map(unsafe_decode::<Row>)
            // If the registration doesn't exists, but a new cc member is being added,
            // then we can initialize a default value.
            .or(match valid_until {
                Resettable::Set(valid_until) => Some(Row {
                    hot_credential: None,
                    valid_until,
                }),
                Resettable::Unchanged | Resettable::Reset => None,
            });

        // Either the cc member exists, or it's a completely new value. Either way, at this point,
        // we do expect a row.
        if let Some(mut row) = row {
            hot_credential.set_or_reset(&mut row.hot_credential);
            db.put(key, as_value(row))
                .map_err(|err| StoreError::Internal(err.into()))?;
        } else {
            // We don't expect modification of cc members to be possible if they don't exists.
            // CC members are added through the ratification of specific governance actions.
            error!(
                target: "store::cc_members::add",
                name = "add.unknown",
                cold_credential.type = %StakeCredentialType::from(&cold_credential),
                cold_credential.hash = %stake_credential_hash(&cold_credential),
            );
        }
    }

    Ok(())
}
