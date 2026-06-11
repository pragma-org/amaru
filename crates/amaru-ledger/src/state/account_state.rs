// Copyright 2026 PRAGMA
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

use crate::{context::AccountState, store::columns::accounts};

/// Raise an account's persisted store row to its materialized validation state. The `rewards`
/// balance is intentionally dropped: it is owned by epoch processing (see `pay_rewards`) and the
/// withdrawals path, not by the validation overlay, so the materialized record only carries the
/// account's registration identity.
impl From<accounts::Row> for AccountState {
    fn from(row: accounts::Row) -> Self {
        AccountState { deposit: row.deposit, pool: row.pool, drep: row.drep }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use amaru_kernel::{StakeCredential, any_stake_credential};
    use proptest::prelude::*;

    use crate::{
        context::{AccountState, Delta},
        state::diff_set::DiffSet,
        store::columns::accounts::tests::any_row,
    };

    proptest! {
        #[test]
        fn accounts_delta_apply_then_undo_restores_base(
            consumed in any_stake_credential(),
            updated in any_stake_credential(),
            base_consumed in any_row(u64::MAX),
            base_updated in any_row(u64::MAX),
            produced in any_row(u64::MAX),
        ) {
            let mut base: BTreeMap<StakeCredential, AccountState> = BTreeMap::new();
            base.insert(consumed.clone(), AccountState::from(base_consumed));
            base.insert(updated.clone(), AccountState::from(base_updated));
            let original = base.clone();

            let mut delta: DiffSet<StakeCredential, AccountState> = DiffSet::default();
            delta.consume(consumed);
            delta.produce(updated, AccountState::from(produced));

            let undo = delta.apply(&mut base);
            undo.apply(&mut base);

            prop_assert_eq!(base, original);
        }
    }
}
