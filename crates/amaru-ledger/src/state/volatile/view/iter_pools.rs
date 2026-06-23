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

use std::{collections::BTreeMap, mem};

use amaru_kernel::{CertificatePointer, Epoch, PoolId, PoolParams};

use crate::{
    state::{diff_epoch_reg::Registrations, volatile::view::DiffEpochReg},
    store::columns::pools::Row as Pool,
};

/// An internal iterator that proxies the stable store's `iter_pools`, but taking into account any
/// pending volatile update.
///
/// NOTE: About 'IterPools'
///
/// This iterator may look complicated, but it exists for two reasons:
///
/// 1. It allows to stick with iterators; which means that the compiler and execution can be
///    optimised towards that. There's no overhead resulting from allocating a large vector of 3000
///    pools. We can intead rely on streaming all the way to construct the updates.
///
/// 2. Until we have hit the stable store, we cannot know whether a registration is actually a new
///    registration, or if it's a re-registration. That's because there's no mechanism to 'update'
///    a pool really, they just re-register. Yet, we don't want to be inspecting each registration
///    independently with a db call because that could be very dramatic.
///
///    Since we can process those updates in any order; we can go ahead and first treat all the
///    database pools, an then, continue the iterator with what's left in the pending state if any.
///    Yet, because of Rust borrowing and ownership model, we cannot just do that on top of the db
///    iterator alone; we must introduce an extra wrapper that will own all the required data and
///    take care of the chaining.
///
/// Importantly, the last points means that there's no guaranteed order on this iterator. Pools
/// shall be considered unordered by consumers of this iterator.
pub(crate) struct IterPools<'volatile, DBIter: Iterator<Item = (PoolId, Pool)>> {
    epoch: Epoch,
    db_iterator: DBIter,
    registrations: BTreeMap<PoolId, Registrations<&'volatile (PoolParams, CertificatePointer)>>,
    retirements: BTreeMap<PoolId, Epoch>,
}

impl<'volatile, DBIter: Iterator<Item = (PoolId, Pool)>> IterPools<'volatile, DBIter> {
    pub fn new(
        epoch: Epoch,
        db_iterator: DBIter,
        pools: &mut DiffEpochReg<PoolId, &'volatile (PoolParams, CertificatePointer)>,
    ) -> Self {
        Self {
            epoch,
            db_iterator,
            registrations: mem::take(&mut pools.registered),
            retirements: mem::take(&mut pools.unregistered),
        }
    }
}

impl<'volatile, DBIter: Iterator<Item = (PoolId, Pool)>> Iterator for IterPools<'volatile, DBIter> {
    type Item = (PoolId, Pool);

    // TODO: reduce logic duplication?
    //
    // - The following code 'patches' the immutable db state with what's transient in the
    //   volatile.
    //
    // - Fundamentally, it duplicates the logic of:
    //   - state::volatile_db::add_pools
    //   - store::columns::pools::extend
    //   - rocksdb::ledger::columns::pools::{add, remove}
    //
    // - However, it doesn't duplicate things in a way that's trivial to unify. But that's
    //   probably something we may want to look into? Perhaps as one of the design goal for a
    //   future ledger store.
    //
    // TODO: annoying clones
    //
    // This also contains a few annoying clones which could likely be avoided or deferred by having
    // the iterator works over a `&Pool`.
    fn next(&mut self) -> Option<Self::Item> {
        // First, we patch stable pools with any pending update
        if let Some((pool_id, mut pool)) = self.db_iterator.next() {
            // Pool is already registered, and has some updates.
            if let Some(update) = self.registrations.remove(&pool_id) {
                let mut future_params =
                    update.into_iter().map(|(pool_params, _)| (Some(pool_params.clone()), self.epoch + 1)).collect();
                pool.future_params.append(&mut future_params);
            }

            // Pool has announced its retirement.
            if let Some(retirement_epoch) = self.retirements.remove(&pool_id) {
                pool.future_params.append(&mut vec![(None, retirement_epoch)])
            }

            return Some((pool_id, pool));
        }

        // Then, we must add any pool that only appears in the volatile
        if let Some((pool_id, registrations)) = self.registrations.pop_first() {
            let (registration, re_registration) = registrations.into_inner();

            let mut pool = Pool::new(registration.1, registration.0.clone());
            if let Some(re_registration) = re_registration {
                pool.future_params = vec![(Some(re_registration.0.clone()), self.epoch + 1)]
            }

            return Some((pool_id, pool));
        }

        None
    }
}
