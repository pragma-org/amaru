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

use std::collections::{BTreeMap, BTreeSet};

use amaru_kernel::{
    Credential, Epoch, Hash, Lovelace, PoolId, PoolMetadata, PoolParams, RationalNumber, RewardAccount,
    utils::string::display_collection,
};
use amaru_observability::{debug, info_span};

use crate::store::columns::pools::Row as Pool;

mod pool_certificates;
pub use pool_certificates::{PendingPoolCertificates, PoolCertificate, PoolCertificates};
#[cfg(any(test, feature = "test-utils"))]
pub use pool_certificates::{any_pool_certificate, any_pool_certificates};

/// Captures stake pool updates computed at the epoch transition, but not yet applied to the
/// immutable storage. Those updates are meant to be updated only after `k` blocks have passed in
/// the following epoch (i.e. once they are stable).
#[derive(Debug, Default)]
pub struct PoolsEpochTransitionUpdates {
    /// Pools that have retired at the epoch transition.
    retired: BTreeSet<PoolId>,

    /// Pools that have updated their parameters and/or metadata at the epoch transition.
    updated: BTreeMap<PoolId, Pool>,

    /// Pool owners refunds, corresponding to the return of their deposit upon de-registration.
    refunds: BTreeMap<Credential, Lovelace>,
}

impl PoolsEpochTransitionUpdates {
    /// Create a new transition update from a read-only store and the epoch that is *beginning*. So
    /// when transitioning from e -> e + 1; 'epoch' is e + 1.
    pub fn new(pools_iter: impl Iterator<Item = (PoolId, Pool)>, epoch: Epoch) -> Self {
        info_span!(ledger::epoch_transition::NEW_POOLS_UPDATES).in_scope(|| {
            let mut pools_updates = Self::default();

            for (_pool_id, pool) in pools_iter {
                pools_updates.tick_pool(epoch, pool);
            }

            pools_updates
        })
    }

    pub fn retired(&self) -> &BTreeSet<PoolId> {
        &self.retired
    }

    pub fn updated(&self) -> &BTreeMap<PoolId, Pool> {
        &self.updated
    }

    pub fn refunds(&self) -> impl Iterator<Item = (&Credential, &Lovelace)> {
        self.refunds.iter()
    }

    /// The pending pool-deposit refund for the given account, or `0`. Refunds land on the reward
    /// balance, so they count towards a withdrawable balance during the straddle.
    pub fn refund(&self, account: &Credential) -> Lovelace {
        self.refunds.get(account).copied().unwrap_or(0)
    }

    /// Only check if a pool would be retiring, without taking ownership or modifying the original
    /// object.
    pub fn is_retiring(epoch: Epoch, pool: &Pool) -> bool {
        pool.pending_certificates.pending_after(epoch).is_retiring()
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
        let pending = pool.pending_certificates.pending_after(epoch);

        // If the most recent retirement is effective as per the current epoch, we simply drop the
        // entry. Note that, any certificate submitted after that retirement would cancel it, which
        // is taken care of in the fold above (clearing 'retirement').
        if pending.is_retiring() {
            return self.retire_pool(epoch, &pool, pending);
        }

        let pool_id = pool.id();

        let has_updated = if let Some(new_params) = pending.registration() {
            // NOTE: hidden exhaustiveness check
            //
            // The following statement is destructuring and not using a wildcard spread `..`
            // *on purpose*. This lets the compiler warns us in case we add new fields to
            // PoolParams.
            let PoolParams { id: _, vrf, pledge, cost, margin, reward_account, owners, relays, metadata } = new_params;

            let current_params = &mut pool.current_params;

            // NOTE: /!\ IMPORTANT /!\ DO NOT INLINE
            //
            // It is tempting to inline all the identifier in the log event below. But don't.
            // This would make the mutation conditioned to the trace severity level. We do need
            // to update the pool parameters irrespective of the severity!
            let vrf = set(&mut current_params.vrf, vrf, Hash::to_string);
            let pledge = set(&mut current_params.pledge, pledge, Lovelace::to_string);
            let cost = set(&mut current_params.cost, cost, Lovelace::to_string);
            let margin = set(&mut current_params.margin, margin, RationalNumber::to_string);
            let reward_account = set(&mut current_params.reward_account, reward_account, RewardAccount::to_string);
            let owners = set(&mut current_params.owners, owners, |s| display_collection(s));
            let relays = set(&mut current_params.relays, relays, |r| display_collection(r));
            let metadata = set(&mut current_params.metadata, metadata, |opt| {
                opt.as_ref().map(PoolMetadata::to_string).unwrap_or_default()
            });

            debug!(
                ledger::epoch_transition::TICK_POOL,
                id = %pool_id,
                @vrf,
                @pledge,
                @cost,
                @margin,
                @reward_account,
                @owners,
                @relays,
                @metadata,
            );

            true
        } else {
            false
        };

        // Regardless, always replace future params with those that remain relevant.
        let has_resolved_certificates = pending.has_resolved_certificates();
        pool.pending_certificates = pending.to_next_certificates();

        if has_updated || has_resolved_certificates {
            self.updated.insert(pool_id, pool);
        }
    }

    fn retire_pool(&mut self, epoch: Epoch, pool: &Pool, pending: PendingPoolCertificates<'_>) {
        debug!(ledger::epoch_transition::RETIRE_POOL, id = %pool.id());

        self.retired.insert(pool.id());
        self.refunds
            .entry(pool.current_params.reward_account.credential())
            .and_modify(|refunded| *refunded += pool.deposit)
            .or_insert(pool.deposit);

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
        assert!(
            pending.is_retiring_at(epoch),
            "invariant violation: no retirement effective exactly at epoch={epoch} survived;\npool={}\ncertificates={:#?}",
            pool.id(),
            pool.pending_certificates,
        );
    }
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
    use amaru_kernel::{
        Epoch, Network, PoolId, PoolParams, RewardAccount, any_certificate_pointer, any_credential, any_lovelace,
        any_pool_params, utils::tests::run_strategy,
    };
    use proptest::{collection::vec, prelude::*};

    use super::{
        PoolCertificate::{self, Registration, Retirement},
        PoolCertificates, PoolsEpochTransitionUpdates,
    };
    use crate::store::columns::pools::Row as Pool;

    // Generate a sequence of plausible updates, where each item in the vector correspond to an
    // epoch's update. So a caller is expected to tick a base Pool between each application.
    pub fn any_row_seq_updates(id: PoolId) -> impl Strategy<Value = Vec<Vec<PoolCertificate>>> {
        vec(Just(()), 0..10).prop_flat_map(move |cols| {
            cols.iter()
                .enumerate()
                .map(|(epoch, _)| {
                    let pending_certificate = || {
                        prop_oneof![
                            (1..3u64).prop_map(move |offset| Retirement(Epoch::from(epoch as u64) + offset)),
                            any_pool_params()
                                .prop_map(move |params| PoolCertificate::from(PoolParams { id, ..params }))
                        ]
                    };
                    vec(pending_certificate(), 0..3)
                })
                .collect::<Vec<_>>()
        })
    }

    #[derive(Debug)]
    struct Model {
        initial_params: PoolParams,
        log: Vec<PoolCertificate>,
        current: Option<PoolParams>,
    }

    impl Model {
        fn new(initial_params: PoolParams) -> Self {
            Self { current: Some(initial_params.clone()), initial_params, log: Vec::new() }
        }

        // Replay the full certificate history rather than modelling any pruning. Applied
        // registrations are permanent: the last one whose epoch has come defines the parameters.
        // Retirements only kill the pool when no certificate was submitted after them.
        fn tick(&mut self, epoch: Epoch, updates: &[PoolCertificate]) {
            self.log.extend(updates.iter().cloned());

            let mut current = Some(self.initial_params.clone());
            let mut retiring = false;
            for certificate in &self.log {
                match certificate {
                    Registration(params) => {
                        current = Some(params.as_ref().clone());
                        retiring = false;
                    }
                    Retirement(at) => retiring = at <= &epoch,
                }
            }

            self.current = if retiring { None } else { current };
        }
    }

    proptest! {
        #[test]
        fn prop_tick_pool(
            registered_at in any_certificate_pointer(u64::MAX),
            deposit in any_lovelace(),
            (initial_params, sequence) in any_pool_params().prop_flat_map(|params| {
                any_row_seq_updates(params.id).prop_map(move |seq| (params.clone(), seq))
            }),
        ) {
            let mut model = Model::new(initial_params.clone());
            let mut pool = Pool::new(registered_at, deposit, initial_params);

            for (submission_epoch, updates) in sequence.into_iter().enumerate() {
                // Certificates submitted during an epoch take effect at the next boundary at the
                // earliest, which is where pools get ticked.
                let boundary = Epoch::from(submission_epoch as u64) + 1;

                model.tick(boundary, &updates);

                let pool_id = pool.id();
                for certificate in updates {
                    pool.pending_certificates.append(certificate);
                }

                let before_tick = pool.clone();
                let mut pools_updates = PoolsEpochTransitionUpdates::default();
                pools_updates.tick_pool(boundary, pool);

                if let Some(updated) = pools_updates.updated().get(&pool_id).cloned() {
                    prop_assert_eq!(
                        model.current.as_ref(),
                        Some(&updated.current_params),
                        "boundary = {:?}, model = {:?}",
                        boundary,
                        model
                    );

                    let has_resolved = updated.pending_certificates.pending_after(boundary).has_resolved_certificates();
                    prop_assert!(
                        !has_resolved,
                        "There can't be any resolved certificates after we cleared the current ones: {:?}",
                        updated.pending_certificates
                    );

                    pool = updated;
                } else if pools_updates.retired().contains(&pool_id) {
                    prop_assert_eq!(
                        model.current.as_ref(),
                        None,
                        "boundary = {:?}, model = {:?}",
                        boundary,
                        model,
                    );
                    break;
                } else {
                    // Nothing took effect at this boundary: the pool must be exactly as it was.
                    prop_assert_eq!(
                        model.current.as_ref(),
                        Some(&before_tick.current_params),
                        "boundary = {:?}, model = {:?}",
                        boundary,
                        model
                    );
                    pool = before_tick;
                }
            }
        }
    }

    proptest! {
        #[test]
        fn prop_pool_stake_deposit(
            registered_at in any_certificate_pointer(u64::MAX),
            deposit in any_lovelace(),
            initial_params in any_pool_params(),
        ) {
            let epoch = Epoch::from(1);
            let reward_account = initial_params.reward_account.credential();

            let mut pool = Pool::new(registered_at, deposit, initial_params);
            let pool_id = pool.id();
            pool.pending_certificates = PoolCertificates::default().with(epoch);

            let mut pools_updates = PoolsEpochTransitionUpdates::default();
            pools_updates.tick_pool(epoch, pool);

            prop_assert!(pools_updates.retired().contains(&pool_id));
            prop_assert_eq!(pools_updates.refund(&reward_account), deposit);
        }
    }

    #[test]
    fn re_registration_cancels_a_later_dated_retirement() {
        let params = run_strategy(any_pool_params());
        let updated_params = PoolParams { pledge: params.pledge.wrapping_add(1), ..params.clone() };

        let mut pool = Pool::new(run_strategy(any_certificate_pointer(u64::MAX)), 500_000_000, params);
        let pool_id = pool.id();

        // A retirement scheduled for a distant epoch, then a re-registration effective sooner. The
        // re-registration cancels the retirement, so the pool must survive the retirement epoch.
        pool.pending_certificates = PoolCertificates::default().with(Epoch::from(617)).with(updated_params.clone());

        let mut at_615 = PoolsEpochTransitionUpdates::default();
        at_615.tick_pool(Epoch::from(615), pool);
        assert!(!at_615.retired().contains(&pool_id), "retired at the re-registration boundary");

        let pool = at_615.updated().get(&pool_id).cloned().expect("parameters update at 615");
        assert_eq!(pool.current_params, updated_params, "new parameters not applied");

        let mut at_617 = PoolsEpochTransitionUpdates::default();
        at_617.tick_pool(Epoch::from(617), pool);
        assert!(!at_617.retired().contains(&pool_id), "cancelled retirement resurfaced at its epoch");
    }

    #[test]
    fn accumulates_refunds_for_multiple_retiring_pools_sharing_a_reward_account() {
        let (mut pool_params_a, mut pool_params_b) = run_strategy(
            (any_pool_params(), any_pool_params())
                .prop_filter("pools must be distinct", |(pool_a, pool_b)| pool_a.id != pool_b.id),
        );
        let reward_credential = run_strategy(any_credential());
        let reward_account = RewardAccount::new(Network::Testnet, reward_credential);

        let deposit_a = 1_000_000;
        let deposit_b = 2_000_000;

        pool_params_a.reward_account = reward_account;
        pool_params_b.reward_account = reward_account;

        let mut pool_a = Pool::new(run_strategy(any_certificate_pointer(u64::MAX)), deposit_a, pool_params_a);
        pool_a.pending_certificates.append(Epoch::from(0));

        let mut pool_b = Pool::new(run_strategy(any_certificate_pointer(u64::MAX)), deposit_b, pool_params_b);
        pool_b.pending_certificates.append(Epoch::from(0));

        let mut pools_updates = PoolsEpochTransitionUpdates::default();
        let pending_a = pool_a.pending_certificates.pending_after(Epoch::from(0));
        let pending_b = pool_b.pending_certificates.pending_after(Epoch::from(0));
        pools_updates.retire_pool(Epoch::from(0), &pool_a, pending_a);
        pools_updates.retire_pool(Epoch::from(0), &pool_b, pending_b);

        let refunds = pools_updates.refunds().collect::<Vec<_>>();
        assert_eq!(refunds, vec![(&reward_credential, &(deposit_a + deposit_b))]);
    }
}
