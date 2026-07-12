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

            // Pool has announced its retirement.
            if let Some(retirement_epoch) = self.retirements.remove(&pool_id) {
                pool.future_params.append(&mut vec![(None, retirement_epoch)])
            }

            return Some((pool_id, pool));
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeMap, BTreeSet},
        sync::LazyLock,
    };

    use amaru_kernel::{any_certificate_pointer, any_pool_params};
    use proptest::{
        strategy::{Strategy, ValueTree},
        test_runner::{Config, RngSeed, TestRunner},
    };
    use test_case::test_case;

    use super::*;
    use crate::epoch_transition::PoolsEpochTransitionUpdates;

    const MAX_POOLS: u8 = 3;

    static STABLE: LazyLock<BTreeMap<u8, (PoolId, Pool)>> = LazyLock::new(|| {
        let row = |ix| {
            let (current_params, registered_at) = mock_pool(ix);
            let future_params = Vec::new();
            (mock_pool_id(ix), Pool { registered_at, current_params, future_params })
        };

        (0..MAX_POOLS).map(|ix| (ix, row(ix))).collect()
    });

    static VOLATILE: LazyLock<BTreeMap<u8, (PoolParams, CertificatePointer)>> = LazyLock::new(|| {
        (0..MAX_POOLS)
            .map(|ix| {
                let (pool_params, registered_at) = mock_pool(u8::MAX - ix);
                (ix, (PoolParams { id: mock_pool_id(ix), ..pool_params }, registered_at))
            })
            .collect()
    });

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Event {
        Registration,
        LateRetirement,
        ImminentRetirement,
    }

    #[derive(Debug, Clone, Copy)]
    struct EndOfEpochView<'a> {
        stable: &'a [(u8, &'a [Event])],
        volatile: &'a [(u8, Event)],
    }

    #[derive(Debug, Clone, Copy)]
    struct NextEpochExpectations<'a> {
        updated: &'a [(u8, Pool)],
        retired: &'a [u8],
        seen: &'a [u8],
    }

    impl From<EndOfEpochView<'_>> for IterPools<'static, std::vec::IntoIter<(PoolId, Pool)>> {
        fn from(test: EndOfEpochView<'_>) -> Self {
            let epoch = Epoch::new(100);

            Self::new(
                epoch,
                test.stable
                    .iter()
                    .map(|(ix, row)| {
                        let (current_params, registered_at) = mock_pool(*ix);
                        let future_params = row
                            .iter()
                            .map(|event| match event {
                                Event::Registration => (Some(mock_pool_params(*ix)), epoch + 1),
                                Event::LateRetirement => (None, epoch + 10),
                                Event::ImminentRetirement => (None, epoch + 1),
                            })
                            .collect();
                        (mock_pool_id(*ix), Pool { registered_at, current_params, future_params })
                    })
                    .collect::<Vec<(PoolId, Pool)>>()
                    .into_iter(),
                &mut test.volatile.iter().fold(DiffEpochReg::default(), |mut diff_epoch_reg, (ix, event)| {
                    let pool_id = mock_pool_id(*ix);

                    match event {
                        Event::Registration => {
                            diff_epoch_reg.register(pool_id, (*VOLATILE).get(ix).unwrap());
                        }
                        Event::LateRetirement => {
                            diff_epoch_reg.unregister(pool_id, epoch + 10);
                        }
                        Event::ImminentRetirement => {
                            diff_epoch_reg.unregister(pool_id, epoch + 1);
                        }
                    }

                    diff_epoch_reg
                }),
            )
        }
    }

    fn mock_pool_id(ix: u8) -> PoolId {
        let mut pool_id = [0; 28];
        pool_id[27] = ix;
        PoolId::from(pool_id)
    }

    fn mock_pool(ix: u8) -> (PoolParams, CertificatePointer) {
        let registered_at = sample(ix, any_certificate_pointer(u64::MAX));
        let pool_params = PoolParams { id: mock_pool_id(ix), ..mock_pool_params(ix) };
        (pool_params, registered_at)
    }

    fn mock_pool_params(ix: u8) -> PoolParams {
        PoolParams { id: mock_pool_id(ix), ..sample(ix, any_pool_params()) }
    }

    fn stable(ix: u8) -> Pool {
        (*STABLE).get(&ix).unwrap().1.clone()
    }

    fn volatile(ix: u8) -> Pool {
        let (current_params, registered_at) = (*VOLATILE).get(&ix).cloned().unwrap();
        Pool { current_params, registered_at, future_params: Vec::new() }
    }

    fn sample<T>(seed: u8, strategy: impl Strategy<Value = T>) -> T {
        let config = Config { failure_persistence: None, rng_seed: RngSeed::Fixed(seed as u64), ..Default::default() };
        strategy
            .new_tree(&mut TestRunner::new(config))
            .unwrap_or_else(|e| panic!("unable to generate arbitrary data: {e}"))
            .current()
    }

    #[test_case(
        EndOfEpochView {
            stable: &[(0, &[]), (1, &[])],
            volatile: &[]
        },
        NextEpochExpectations {
            seen: &[0, 1],
            updated: &[],
            retired: &[],
        };
        "no volatile, no updates, no retirements"
    )]
    #[test_case(
        EndOfEpochView {
            stable: &[],
            volatile: &[(0, Event::Registration)]
        },
        NextEpochExpectations {
            seen: &[0],
            updated: &[],
            retired: &[],
        };
        "no stable, one volatile registration"
    )]
    #[test_case(
        EndOfEpochView {
            stable: &[(0, &[])],
            volatile: &[(0, Event::Registration)]
        },
        NextEpochExpectations {
            seen: &[0],
            updated: &[(0, Pool { current_params: volatile(0).current_params, ..stable(0) })],
            retired: &[]
        };
        "existing stable, one volatile update"
    )]
    #[test_case(
        EndOfEpochView {
            stable: &[(0, &[Event::Registration])],
            volatile: &[]
        },
        NextEpochExpectations {
            seen: &[0],
            updated: &[(0, stable(0))],
            retired: &[]
        };
        "existing stable, one stable update"
    )]
    #[test_case(
        EndOfEpochView {
            stable: &[(0, &[Event::Registration])],
            volatile: &[(0, Event::Registration)]
        },
        NextEpochExpectations {
            seen: &[0],
            // Volatile wins
            updated: &[(0, Pool { current_params: volatile(0).current_params, ..stable(0) })],
            retired: &[]
        };
        "existing stable, one stable update, one volatile update"
    )]
    #[test_case(
        EndOfEpochView {
            stable: &[(0, &[])],
            volatile: &[(0, Event::ImminentRetirement)]
        },
        NextEpochExpectations {
            seen: &[0],
            updated: &[],
            retired: &[0]
        };
        "existing stable, imminent retirement in volatile"
    )]
    #[test_case(
        EndOfEpochView {
            stable: &[(0, &[])],
            volatile: &[(0, Event::LateRetirement)]
        },
        NextEpochExpectations {
            seen: &[0],
            // No update at the epoch boundary, the retirement certificate will eventually be
            // inserted but not as a result of crossing the epoch boundary. By the time we
            // flush the update to the store, it will already be there, so there's no need to
            // re-insert it or clean up the future parameters.
            updated: &[],
            retired: &[]
        };
        "existing stable, late retirement in volatile"
    )]
    #[test_case(
        EndOfEpochView {
            stable: &[(0, &[Event::Registration])],
            volatile: &[(0, Event::LateRetirement)]
        },
        NextEpochExpectations {
            seen: &[0],
            // Unlike the previous case, we have to clean-up the re-registration from the future
            // parameters and yet, preserve the late retirement as well. There's no diff strategy on
            // pool updates, we just replace the pool object entirely; and must therefore include
            // the late retirement.
            updated: &[(0, Pool { future_params: vec![(None, Epoch::from(110))], ..stable(0) })],
            retired: &[]
        };
        "update in stable, late retirement in volatile"
    )]
    #[test_case(
        EndOfEpochView {
            stable: &[(0, &[Event::Registration])],
            volatile: &[(0, Event::ImminentRetirement)]
        },
        NextEpochExpectations {
            seen: &[0],
            updated: &[],
            retired: &[0]
        };
        "update in stable, imminent retirement in volatile"
    )]
    #[test_case(
        EndOfEpochView {
            stable: &[(0, &[Event::ImminentRetirement])],
            volatile: &[(0, Event::Registration)]
        },
        NextEpochExpectations {
            seen: &[0],
            updated: &[(0, Pool { current_params: volatile(0).current_params, ..stable(0) })],
            retired: &[]
        };
        "imminent retirement in stable, update in volatile"
    )]
    #[test_case(
        EndOfEpochView {
            stable: &[(0, &[Event::ImminentRetirement])],
            volatile: &[(0, Event::LateRetirement)]
        },
        NextEpochExpectations {
            seen: &[0],
            updated: &[(0, Pool { future_params: vec![(None, Epoch::from(110))], ..stable(0) })],
            retired: &[]
        };
        "imminent retirement in stable, late retirement in volatile"
    )]
    #[test_case(
        EndOfEpochView {
            stable: &[],
            volatile: &[(0, Event::Registration), (0, Event::ImminentRetirement)]
        },
        NextEpochExpectations {
            seen: &[0],
            updated: &[],
            retired: &[0]
        };
        "registration in volatile, imminent retirement in volatile"
    )]
    #[test_case(
        EndOfEpochView {
            stable: &[],
            volatile: &[(0, Event::Registration), (0, Event::LateRetirement)]
        },
        NextEpochExpectations {
            seen: &[0],
            updated: &[],
            retired: &[]
        };
        "registration in volatile, late retirement in volatile"
    )]
    #[test_case(
        EndOfEpochView {
            stable: &[],
            volatile: &[(0, Event::Registration), (0, Event::Registration)]
        },
        NextEpochExpectations {
            seen: &[0],
            updated: &[(0, volatile(0))],
            retired: &[]
        };
        "registration in volatile, re-registration in volatile"
    )]
    #[test_case(
        EndOfEpochView {
            stable: &[(0, &[])],
            volatile: &[(0, Event::Registration), (0, Event::Registration)]
        },
        NextEpochExpectations {
            seen: &[0],
            updated: &[(0, Pool { current_params: volatile(0).current_params, ..stable(0) })],
            retired: &[]
        };
        "existing stable, registration in volatile, re-registration in volatile"
    )]
    fn iter_pools_scenarios(test: EndOfEpochView<'_>, expectations: NextEpochExpectations<'_>) {
        let iterator = IterPools::from(test);
        let epoch = iterator.epoch;

        let mut seen = BTreeSet::new();
        let transition = PoolsEpochTransitionUpdates::new(
            iterator.map(|(ix, pool)| {
                seen.insert(ix);
                (ix, pool)
            }),
            epoch + 1,
        );

        assert_eq!(expectations.seen.iter().copied().map(mock_pool_id).collect::<BTreeSet<_>>(), seen, "seen mismatch");

        let retired = transition.retired();
        for ix in expectations.retired {
            assert!(retired.contains(&mock_pool_id(*ix)), "missing retired");
        }
        assert_eq!(expectations.retired.len(), retired.len(), "retired mismatch");

        let updated = transition.updated();
        for (ix, pool) in expectations.updated {
            assert_eq!(Some(pool), updated.get(&mock_pool_id(*ix)), "updated mismatch")
        }
        assert_eq!(expectations.updated.len(), updated.len(), "updated mismatch");
    }
}
