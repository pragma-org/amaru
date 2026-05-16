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
    ops::Deref,
};

use amaru_kernel::{
    Epoch, Hash, Lovelace, PoolId, PoolParams, RewardAccount, StakeCredential, expect_stake_credential, hash,
    pool_metadata, rational_number, relay,
};
use tracing::debug;

use crate::store::columns::pools;

/// Captures stake pool updates computed at the epoch transition, but not yet applied to the
/// immutable storage. Those updates are meant to be updated only after `k` blocks have passed in
/// the following epoch (i.e. once they are stable).
#[derive(Debug, Default)]
pub struct PoolsEpochTransitionUpdates {
    /// Pools that have retired at the epoch transition.
    retired: BTreeSet<PoolId>,

    /// Pools that have updated their parameters and/or metadata at the epoch transition.
    updated: BTreeMap<PoolId, pools::Row>,

    /// Pool owners refunds, corresponding to the return of their deposit upon de-registration.
    refunds: BTreeMap<StakeCredential, Lovelace>,
}

const STAKE_POOL_DEPOSIT: Lovelace = 500_000_000;

impl PoolsEpochTransitionUpdates {
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
    pub fn tick_pool(&mut self, epoch: Epoch, mut pool: pools::Row) {
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

                debug!(
                    pool = %pool_id,
                    vrf = set(&mut current_params.vrf, vrf, Hash::to_string),
                    pledge = set(&mut current_params.pledge, pledge, Lovelace::to_string),
                    cost = set(&mut current_params.cost, cost, Lovelace::to_string),
                    margin = set(&mut current_params.margin, margin, rational_number::fmt),
                    reward_account = set(&mut current_params.reward_account, reward_account, RewardAccount::to_string),
                    owners = set(&mut current_params.owners, owners, |s| hash::fmt(s.deref())),
                    relays = set(&mut current_params.relays, relays, |r| relay::fmt(r)),
                    metadata = set(&mut current_params.metadata, metadata, pool_metadata::fmt),
                    "tick.pool.updating",
                );
            }

            // Regardless, always prune future params from those that are now-obsolete.
            pool.future_params.retain(|(_, effective_in)| effective_in > &epoch);

            self.updated.insert(pool_id, pool);
        }
    }

    fn retire_pool(&mut self, epoch: Epoch, pool: pools::Row) {
        debug!(pool = %pool.id(), "tick.pool.retiring");

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
