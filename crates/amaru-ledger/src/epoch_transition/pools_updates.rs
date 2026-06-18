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

use std::{
    collections::{BTreeMap, BTreeSet},
    mem,
    ops::Deref,
};

use amaru_kernel::{
    Epoch, Hash, Lovelace, PoolId, PoolParams, RewardAccount, StakeCredential, expect_stake_credential, hash,
    pool_metadata, rational_number, relay,
};
use amaru_observability::info_span;
use tracing::debug;

use crate::store::columns::pools::Row as Pool;

/// Captures stake pool updates computed at the epoch transition, but not yet applied to the
/// immutable storage. Those updates are meant to be updated only after `k` blocks have passed in
/// the following epoch (i.e. once they are stable).
#[derive(Debug, Default, Clone)]
pub struct PoolsEpochTransitionUpdates {
    /// Pools that have retired at the epoch transition.
    retired: BTreeSet<PoolId>,

    /// Pools that have updated their parameters and/or metadata at the epoch transition.
    updated: BTreeMap<PoolId, Pool>,

    /// Pool owners refunds, corresponding to the return of their deposit upon de-registration.
    refunds: BTreeMap<StakeCredential, Lovelace>,
}

const STAKE_POOL_DEPOSIT: Lovelace = 500_000_000;

impl PoolsEpochTransitionUpdates {
    /// Create a new transition update from a read-only store and the epoch that is *beginning*. So
    /// when transitioning from e -> e + 1; 'epoch' is e + 1.
    pub fn new(pools_iter: impl Iterator<Item = (PoolId, Pool)>, epoch: Epoch) -> Self {
        info_span!(amaru_observability::amaru::ledger::epoch_transition::NEW_POOLS_UPDATES).in_scope(|| {
            let mut pools_updates = Self::default();

            for (_pool_id, pool) in pools_iter {
                pools_updates.tick_pool(epoch, pool)
            }

            pools_updates
        })
    }

    pub fn retired(&self) -> &BTreeSet<PoolId> {
        &self.retired
    }

    pub fn take_retired(&mut self) -> BTreeSet<PoolId> {
        mem::take(&mut self.retired)
    }

    pub fn updated(&self) -> &BTreeMap<PoolId, Pool> {
        &self.updated
    }

    pub fn take_updated(&mut self) -> BTreeMap<PoolId, Pool> {
        mem::take(&mut self.updated)
    }

    pub fn refunds(&self) -> impl Iterator<Item = (&StakeCredential, Lovelace)> {
        self.refunds.iter().map(|(account, refund)| (account, *refund))
    }

    /// Only check if a pool would be retiring, without taking ownership or modifying the original
    /// object.
    pub fn is_retiring(epoch: Epoch, pool: &Pool) -> bool {
        let (_, retirement, needs_update) = fold_future_params(&pool.future_params, epoch);
        needs_update && retirement.is_some_and(|retirement_epoch| retirement_epoch <= epoch)
    }

    /// Check whether a pool needs any sort of updates at the beginning of an epoch
    /// ('current_epoch').
    ///
    /// A pool can have two types of updates:
    ///
    /// 1. Re-registration (effectively adjusting its underlying parameters or metadata), which
    ///    always take effect at the beginning of the following epoch where the update happen.
    ///
    /// 2. Retirements, which specifies an epoch where the retirement becomes effective. Pools are
    ///    retired at the beginning of epochs.
    ///
    /// During an epoch, we collect all updates as they arrive from blocks. We then fold over those
    /// updates in this function, following a couple of rules:
    ///
    /// a. Any re-registration that comes after a retirement cancels that retirement.
    /// b. Any retirement that come after a retirement cancels that previous retirement.
    pub fn tick_pool(&mut self, epoch: Epoch, mut pool: Pool) {
        let (update, retirement, needs_update) = fold_future_params(&pool.future_params, epoch);

        if needs_update {
            // If the most recent retirement is effective as per the current epoch, we simply drop the
            // entry. Note that, any re-registration happening after that retirement would cancel it,
            // which is taken care of in the fold above (returning 'None').
            if let Some(retirement_epoch) = retirement
                && retirement_epoch <= epoch
            {
                return self.retire_pool(epoch, pool);
            }

            let pool_id = pool.id();

            if let Some(new_params) = update {
                // NOTE: hidden exhaustiveness check
                //
                // The following statement is destructuring and not using a wildcard spread `..`
                // *on purpose*. This lets the compiler warns us in case we add new fields to
                // PoolParams.
                let PoolParams { id: _, vrf, pledge, cost, margin, reward_account, owners, relays, metadata } =
                    new_params;

                let current_params = &mut pool.current_params;

                // NOTE: /!\ IMPORTANT /!\ DO NOT INLINE
                //
                // It is tempting to inline all the identifier in the log event below. But don't.
                // This would make the mutation conditioned to the trace severity level. We do need
                // to update the pool parameters irrespective of the severity!
                let vrf = set(&mut current_params.vrf, vrf, Hash::to_string);
                let pledge = set(&mut current_params.pledge, pledge, Lovelace::to_string);
                let cost = set(&mut current_params.cost, cost, Lovelace::to_string);
                let margin = set(&mut current_params.margin, margin, rational_number::fmt);
                let reward_account = set(&mut current_params.reward_account, reward_account, RewardAccount::to_string);
                let owners = set(&mut current_params.owners, owners, |s| hash::fmt(s.deref()));
                let relays = set(&mut current_params.relays, relays, |r| relay::fmt(r));
                let metadata = set(&mut current_params.metadata, metadata, pool_metadata::fmt);

                debug!(
                    name: "pool.update",
                    id = %pool_id,
                    vrf,
                    pledge,
                    cost,
                    margin,
                    reward_account,
                    owners,
                    relays,
                    metadata,
                );
            }

            // Regardless, always prune future params from those that are now-obsolete.
            pool.future_params.retain(|(_, effective_in)| effective_in > &epoch);

            self.updated.insert(pool_id, pool);
        }
    }

    fn retire_pool(&mut self, epoch: Epoch, pool: Pool) {
        debug!(name: "pool.retire", id = %pool.id());

        self.retired.insert(pool.id());
        self.refunds.insert(
            expect_stake_credential(&pool.current_params.reward_account),
            // FIXME: Store stake pool deposit when registering pools
            //
            // The stake pool deposit is a protocol parameter which may get updated between
            // the moment a pool registers for the first time. Then, when de-registering,
            // we must simply refer to that amount, irrespective of the current protocol
            // parameter value.
            STAKE_POOL_DEPOSIT,
        );

        // NOTE: Sanity check on pool retirement
        //
        // Callee shall ensure that all pools are ticked on epoch-boundaries.
        //
        // Hence, since:
        //
        // 1. Re-registrations can only be scheduled for next epoch;
        // 2. Re-registrations cancel out any retirement for the same epoch;
        // 3. Retirements cancel out any retirement scheduled and not yet enacted.
        //
        // Then we cannot find a case where a pool retires and still have a
        // re-registration or another retirement still scheduled. Note that the reason
        // we enforce this invariant here is because the next action will erase the
        // pool -- and any remaining updates with it. This would have dramatic
        // consequences should we still have updates stashed for the future.
        let last = pool.future_params.last();
        assert_eq!(
            last,
            Some(&(None, epoch)),
            "invariant violation: most recent retirement is not last certificate: {:?}",
            last,
        );
    }
}

/// Collapse stake pool future parameters according to the current epoch. The stable DB is at most k
/// blocks in the past. So, if a certificate is submitted near the end (i.e. within k blocks) of the
/// last epoch, then we could be in a situation where we haven't yet processed the registrations
/// (since they're processed with a delay of k blocks) but have already moved into the next epoch.
///
/// The function returns any new params becoming active in the 'current_epoch', and the retirement
/// status of the pool. Note that the pool can both have new parameters AND a retirement scheduled
/// at a later epoch.
///
/// The boolean indicates whether any of the future params are now-obsolete as per the
/// 'current_epoch'.
pub fn fold_future_params(
    future_params: &[(Option<PoolParams>, Epoch)],
    current_epoch: Epoch,
) -> (Option<&PoolParams>, Option<Epoch>, bool) {
    future_params.iter().fold((None, None, false), |(update, retirement, needs_update), (params, epoch)| {
        match params {
            // Pool has a parameter update that should now be applied.
            Some(params) if epoch <= &current_epoch => (Some(params), None, true),
            // Pool has a parameter update for another future epoch.
            Some(..) => (update, retirement, needs_update),
            // Pool is retiring *now*
            None if epoch <= &current_epoch => (None, Some(*epoch), true),
            // Pool is retiring later.
            None => (update, Some(*epoch), needs_update),
        }
    })
}

// Update a value in a source object, and returns a tracing field ready to be displayed. The field
// is empty in case there's no update.
fn set<A: Eq + Clone>(source: &mut A, new: &A, to_string: impl FnOnce(&A) -> String) -> Box<dyn tracing::Value> {
    if source != new {
        let field = to_string(new);
        *source = new.clone();
        Box::new(field)
    } else {
        Box::new(tracing::field::Empty) as Box<dyn tracing::Value>
    }
}

#[cfg(test)]
mod tests {
    use amaru_kernel::{Epoch, PoolParams, any_certificate_pointer, any_pool_params};
    use proptest::{collection::vec, prelude::*};

    use super::PoolsEpochTransitionUpdates;
    use crate::store::columns::pools::Row as Pool;

    // Generate a sequence of plausible updates, where each item in the vector correspond to an
    // epoch's update. So a caller is expected to tick a base Pool between each application.
    pub fn any_row_seq_updates() -> impl Strategy<Value = Vec<Vec<(Option<PoolParams>, Epoch)>>> {
        vec(Just(()), 0..10).prop_flat_map(|cols| {
            cols.iter()
                .enumerate()
                .map(|(epoch, _)| {
                    let future_params = || {
                        prop_oneof![
                            (1..3u64).prop_map(move |offset| (None, Epoch::from(epoch as u64) + offset)),
                            any_pool_params().prop_map(move |params| (Some(params), Epoch::from(epoch as u64 + 1)))
                        ]
                    };
                    vec(future_params(), 0..3)
                })
                .collect::<Vec<_>>()
        })
    }

    #[derive(Debug)]
    struct Model {
        current: Option<PoolParams>,
        future: Option<PoolParams>,
        retiring: Option<Epoch>,
    }

    impl Model {
        fn new(initial_params: PoolParams) -> Self {
            Self { current: Some(initial_params), future: None, retiring: None }
        }

        // Apply model's changes at the epoch boundary
        fn begin_epoch(&mut self, epoch: Epoch) {
            if let Some(retirement) = self.retiring
                && retirement <= epoch
            {
                self.current = None;
            }

            if let Some(future) = self.future.take() {
                self.current = Some(future);
            }
        }

        // Process all updates through our simpler model
        fn tick(&mut self, epoch: Epoch, updates: &[(Option<PoolParams>, Epoch)]) {
            self.begin_epoch(epoch);

            for (update, retirement_epoch) in updates {
                match update {
                    None if self.current.is_none() => {}
                    None => {
                        self.retiring = Some(*retirement_epoch);
                    }
                    Some(params) if self.current.is_none() => {
                        self.retiring = None;
                        self.current = Some(params.clone());
                    }
                    Some(params) => {
                        self.retiring = None;
                        self.future = Some(params.clone());
                    }
                }
            }
        }
    }

    proptest! {
        #[test]
        fn prop_tick_pool(
            registered_at in any_certificate_pointer(u64::MAX),
            initial_params in any_pool_params(),
            sequence in any_row_seq_updates(),
        ) {
            let mut model = Model::new(initial_params.clone());
            let mut pool_opt = Some(Pool::new(registered_at, initial_params));

            for (current_epoch, updates) in sequence.into_iter().enumerate() {
                let Some(mut pool) = pool_opt.take() else {
                    break;
                };

                let current_epoch = Epoch::from(current_epoch as u64);

                model.tick(current_epoch, &updates);

                let mut pools_updates = PoolsEpochTransitionUpdates::default();
                let pool_id = pool.id();
                pool.future_params = updates;
                pools_updates.tick_pool(current_epoch, pool);

                if let Some(pool) = pools_updates.updated().get(&pool_id).cloned() {
                    prop_assert_eq!(
                        model.current.as_ref(),
                        Some(&pool.current_params),
                        "current_epoch = {:?}, model = {:?}",
                        current_epoch,
                        model
                    );

                    let obsolete_count = pool.future_params.iter()
                        .filter(|(_, epoch)| epoch <= &current_epoch)
                        .count();

                    prop_assert_eq!(
                        obsolete_count,
                        0,
                        "future_params should not contain obsolete entries: {:?}",
                        pool.future_params
                    );

                    pool_opt = Some(pool)
                } else if pools_updates.retired().contains(&pool_id) {
                    prop_assert_eq!(
                        model.current.as_ref(),
                        None,
                        "current_epoch = {:?}, model = {:?}",
                        current_epoch,
                        model,
                    );
                }
            }
        }
    }
}
