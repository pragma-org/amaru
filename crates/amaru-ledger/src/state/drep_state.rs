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

use amaru_kernel::{Anchor, CertificatePointer, Epoch, Lovelace};

use crate::store::columns::dreps;

/// The materialized state of a single delegate representative, as the volatile ledger aggregate sees it.
///
/// Like the pool and account records, this mirrors the persisted drep row but lives in the ledger
/// layer; free of the store's serialization concerns.
#[derive(Debug, Clone, PartialEq)]
pub struct DRepState {
    pub deposit: Lovelace,
    pub anchor: Option<Anchor>,
    pub registered_at: CertificatePointer,
    pub valid_until: Epoch,
}

/// Raise a drep's persisted store row to its materialized state.
impl From<dreps::Row> for DRepState {
    fn from(row: dreps::Row) -> Self {
        DRepState {
            deposit: row.deposit,
            anchor: row.anchor,
            registered_at: row.registered_at,
            valid_until: row.valid_until,
        }
    }
}

/// Lower a drep's materialized state back to its persisted store row.
impl From<DRepState> for dreps::Row {
    fn from(state: DRepState) -> Self {
        dreps::Row {
            deposit: state.deposit,
            anchor: state.anchor,
            registered_at: state.registered_at,
            valid_until: state.valid_until,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use amaru_kernel::{StakeCredential, any_stake_credential};
    use proptest::prelude::*;

    use super::*;
    use crate::{
        context::Delta,
        state::diff_set::DiffSet,
        store::columns::dreps::{Row, tests::any_row},
    };

    proptest! {
        #[test]
        fn row_state_roundtrip(row in any_row(u64::MAX)) {
            let back: Row = DRepState::from(row.clone()).into();
            prop_assert_eq!(back, row);
        }
    }

    proptest! {
        #[test]
        fn dreps_delta_apply_then_undo_restores_base(
            consumed in any_stake_credential(),
            updated in any_stake_credential(),
            base_consumed in any_row(u64::MAX),
            base_updated in any_row(u64::MAX),
            produced in any_row(u64::MAX),
        ) {
            let mut base: BTreeMap<StakeCredential, DRepState> = BTreeMap::new();
            base.insert(consumed.clone(), DRepState::from(base_consumed));
            base.insert(updated.clone(), DRepState::from(base_updated));
            let original = base.clone();

            let mut delta: DiffSet<StakeCredential, DRepState> = DiffSet::default();
            delta.consume(consumed);
            delta.produce(updated, DRepState::from(produced));

            let undo = delta.apply(&mut base);
            undo.apply(&mut base);

            prop_assert_eq!(base, original);
        }
    }
}
