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

use amaru_kernel::{CertificatePointer, Epoch, PoolParams};

use crate::{epoch_transition::fold_future_params, store::columns::pools};

/// The materialized state of a single stake pool, as the volatile ledger overlay sees it.
///
/// This mirrors the volatile-relevant fields of the persisted pool row, but lives in the ledger
/// layer; free of the store's serialization concerns. Re-registrations (parameter updates) and
/// retirements accumulate in `future_params` as they arrive from blocks; both are deferred and only
/// enacted at an epoch boundary, via [`PoolState::tick`].
///
/// The invertible pools delta is therefore an upsert/remove map over this type, i.e.
/// `DiffSet<PoolId, PoolState>`.
#[derive(Debug, Clone, PartialEq)]
pub struct PoolState {
    pub registered_at: CertificatePointer,
    pub current_params: PoolParams,
    pub future_params: Vec<(Option<PoolParams>, Epoch)>,
}

/// The outcome of enacting a pool's scheduled changes at the beginning of an epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PoolEnactment {
    /// The pool remains registered; its parameters may have been updated.
    Active,
    /// The pool has retired and should be removed.
    Retired,
}

impl PoolState {
    /// Create the state for a freshly-registered pool.
    pub fn new(registered_at: CertificatePointer, current_params: PoolParams) -> Self {
        Self { registered_at, current_params, future_params: Vec::new() }
    }

    /// Schedule a re-registration (parameter update), to take effect at the start of `effective`.
    ///
    /// Re-registrations always take effect on a later epoch; the update is staged in
    /// `future_params` until [`PoolState::tick`] enacts it.
    pub fn register(&mut self, params: PoolParams, effective: Epoch) {
        self.future_params.push((Some(params), effective));
    }

    /// Schedule a retirement, to take effect at the start of `epoch`.
    pub fn retire(&mut self, epoch: Epoch) {
        self.future_params.push((None, epoch));
    }

    /// Enact any scheduled changes that have come due as of `epoch`, returning whether the pool
    /// survives.
    ///
    /// Folding the staged updates preserves the lifecycle invariants: a re-registration cancels an
    /// earlier retirement, a later retirement supersedes an earlier one, and updates scheduled for
    /// a future epoch are left in place.
    pub fn tick(&mut self, epoch: Epoch) -> PoolEnactment {
        let (update, retirement, needs_update) = fold_future_params(&self.future_params, epoch);
        let new_params = update.cloned();
        let retiring = retirement.is_some_and(|retirement_epoch| retirement_epoch <= epoch);

        if !needs_update {
            return PoolEnactment::Active;
        }

        if retiring {
            return PoolEnactment::Retired;
        }

        if let Some(new_params) = new_params {
            self.current_params = new_params;
        }

        self.future_params.retain(|(_, effective_in)| effective_in > &epoch);

        PoolEnactment::Active
    }
}

/// Raise a pool's persistated store row to its materialized state.
impl From<pools::Row> for PoolState {
    fn from(row: pools::Row) -> Self {
        PoolState {
            registered_at: row.registered_at,
            current_params: row.current_params,
            future_params: row.future_params,
        }
    }
}

/// Lower a pool's materialized state back to its persisted store row.
impl From<PoolState> for pools::Row {
    fn from(state: PoolState) -> Self {
        pools::Row {
            registered_at: state.registered_at,
            current_params: state.current_params,
            future_params: state.future_params,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use amaru_kernel::{Epoch, PoolId, any_certificate_pointer, any_pool_params};
    use proptest::prelude::*;

    use super::*;
    use crate::{
        context::Delta,
        state::diff_set::DiffSet,
        store::columns::pools::{Row, tests::any_row},
    };

    proptest! {
        #[test]
        fn register_takes_effect_next_epoch(
            registered_at in any_certificate_pointer(u64::MAX),
            initial in any_pool_params(),
            updated in any_pool_params(),
        ) {
            let mut pool = PoolState::new(registered_at, initial.clone());
            pool.register(updated.clone(), Epoch::from(5));

            prop_assert_eq!(pool.tick(Epoch::from(4)), PoolEnactment::Active);
            prop_assert_eq!(&pool.current_params, &initial, "update is not yet effective at epoch 4");

            prop_assert_eq!(pool.tick(Epoch::from(5)), PoolEnactment::Active);
            prop_assert_eq!(&pool.current_params, &updated, "update becomes effective at epoch 5");
        }
    }

    proptest! {
        #[test]
        fn retire_enacts_at_scheduled_epoch(
            registered_at in any_certificate_pointer(u64::MAX),
            params in any_pool_params(),
        ) {
            let mut pool = PoolState::new(registered_at, params);
            pool.retire(Epoch::from(5));

            prop_assert_eq!(pool.tick(Epoch::from(4)), PoolEnactment::Active);
            prop_assert_eq!(pool.tick(Epoch::from(5)), PoolEnactment::Retired);
        }
    }

    proptest! {
        #[test]
        fn re_registration_cancels_retirement(
            registered_at in any_certificate_pointer(u64::MAX),
            initial in any_pool_params(),
            updated in any_pool_params(),
        ) {
            let mut pool = PoolState::new(registered_at, initial);
            pool.retire(Epoch::from(5));
            pool.register(updated.clone(), Epoch::from(5));

            prop_assert_eq!(pool.tick(Epoch::from(5)), PoolEnactment::Active);
            prop_assert_eq!(&pool.current_params, &updated);
        }
    }

    proptest! {
        #[test]
        fn later_retirement_supersedes_earlier(
            registered_at in any_certificate_pointer(u64::MAX),
            params in any_pool_params(),
        ) {
            let mut pool = PoolState::new(registered_at, params);
            pool.retire(Epoch::from(5));
            pool.retire(Epoch::from(7));

            prop_assert_eq!(pool.tick(Epoch::from(5)), PoolEnactment::Active);
            prop_assert_eq!(pool.tick(Epoch::from(7)), PoolEnactment::Retired);
        }
    }

    proptest! {
        #[test]
        fn pools_delta_apply_then_undo_restores_base(
            registered_at in any_certificate_pointer(u64::MAX),
            a in any_pool_params(),
            b in any_pool_params(),
            c in any_pool_params(),
        ) {
            let mut base: BTreeMap<PoolId, PoolState> = BTreeMap::new();
            base.insert(a.id, PoolState::new(registered_at, a.clone()));
            base.insert(b.id, PoolState::new(registered_at, b.clone()));
            let original = base.clone();

            let mut delta: DiffSet<PoolId, PoolState> = DiffSet::default();
            delta.consume(a.id);
            delta.produce(c.id, PoolState::new(registered_at, c));

            let undo = delta.apply(&mut base);
            undo.apply(&mut base);

            prop_assert_eq!(base, original);
        }
    }

    proptest! {
        #[test]
        fn row_state_roundtrip(row in any_row()) {
            let back: Row = PoolState::from(row.clone()).into();
            prop_assert_eq!(back, row);
        }
    }
}
