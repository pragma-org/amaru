// Copyright 2024 PRAGMA
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
    borrow::Cow,
    cmp::max,
    collections::{BTreeMap, BTreeSet, VecDeque},
    net::SocketAddr,
    ops::Deref,
    sync::{Arc, Mutex, MutexGuard},
};

use amaru_kernel::{
    Block, Epoch, EraHistory, EraHistoryError, GlobalParameters, HasTransactionId, Hash, Hasher, NetworkName, Point,
    PoolId, ProtocolParameters, Slot, Tip, Transaction, TransactionId, TransactionPointer, protocol_version, to_cbor,
    utils::string::display_collection,
};
use amaru_metrics::ledger::LedgerMetrics;
use amaru_observability::{debug_span, info, info_span, trace, warn};
use amaru_ouroboros_traits::{PoolSummaries, PoolSummary};
use amaru_plutus::arena_pool::ArenaPool;
use num::CheckedSub;
use thiserror::Error;
use tracing::Span;

use crate::{
    context::{ContextHydratationError, DefaultPreparationContext, DefaultValidationContext, UnresolvedInputPolicy},
    epoch_transition::{Effective, GovernanceActivity, GovernanceUpdates, PoolsEpochTransitionUpdates, Rewards},
    governance::ratification::RatificationContext,
    rules::{
        self,
        block::{BlockValidation, TransactionInvalid},
    },
    state::volatile::{
        AnchoredVolatileFragment, StoreUpdate, VolatileDB, VolatileFragment, VolatileSequence, VolatileView,
    },
    store::{HistoricalStores, Snapshot, Store, StoreError, TransactionalContext},
    summary::{
        governance::{self, GovernanceSummary},
        rewards::RewardsSummary,
        stake_distribution::StakeDistribution,
    },
    tracing_enabled,
};

pub mod diff_bind;
pub mod diff_epoch_reg;
pub mod diff_set;
pub mod volatile;

/// The minimum number of past (from the current epoch) snapshots required for the ledger to
/// operate.
pub const MIN_LEDGER_SNAPSHOTS: u64 = 3;

// State
// ----------------------------------------------------------------------------

/// The state of the ledger split into two sub-components:
///
/// - A _stable_ and persistent storage, which contains the part of the state which known to be
///   final. Fundamentally, this contains the aggregated state of the ledger that is at least 'k'
///   blocks old; where 'k' is the security parameter of the protocol.
///
/// - A _volatile_ state, which is maintained as a sequence of diff operations to be applied on
///   top of the _stable_ store. It contains at most 'GlobalParameters::consensus_security_param' entries; old entries
///   get persisted in the stable storage when they are popped out of the volatile state.
pub struct State<S, HS>
where
    S: Store,
    HS: HistoricalStores,
{
    /// A handle to the stable store, shared across all ledger instances.
    stable: Arc<Mutex<S>>,

    /// A handle to the stable store, shared across all ledger instances.
    snapshots: HS,

    /// Our own in-memory vector of volatile deltas to apply onto the stable store in due time.
    volatile: VolatileDB,

    /// Global (i.e. non-updatable) parameters of the network. This includes things like
    /// slot length, epoch length, security parameter and other pieces that cannot generally
    /// be updated but grouped here to avoid dealing with magic values everywhere.
    global_parameters: Arc<GlobalParameters>,

    /// A (shared) collection of the latest stake distributions. Those are used both during rewards
    /// calculations, and for leader schedule verification.
    ///
    /// TODO: StakeDistribution are relatively large objects that typically present a lot of
    /// duplications. We won't usually store more than 3 of them at the same time, since we get rid
    /// of them when no longer needed (after rewards calculations).
    ///
    /// Yet, we could imagine a more compact representation where keys for pool and accounts
    /// wouldn't be so much duplicated between snapshots. Instead, we could use an array of values
    /// for each key. On a distribution of 1M+ stake credentials, that's ~26MB of memory per
    /// duplicate.
    stake_distributions: Arc<Mutex<VecDeque<StakeDistribution>>>,

    /// The era history for the network this store is related to.
    era_history: Arc<EraHistory>,

    /// Which network are we connected to. This is mostly helpful for distinguishing between
    /// behavious that are network specifics (e.g. address discriminant).
    network: NetworkName,

    /// Optional callback invoked whenever a new stake distribution snapshot is added.
    /// Used to update resources and notify stages (e.g. track_peers) about fresh PoolSummaries.
    on_stake_dist_updated: Option<Arc<dyn Fn(PoolSummaries) + Send + Sync>>,
}

impl<S: Store, HS: HistoricalStores> State<S, HS> {
    /// The last known epoch; or said differently, the epoch the volatile overlay is valid for.
    pub fn epoch(&self) -> Epoch {
        self.volatile.epoch()
    }

    /// Get the current protocol version, applying any pending overlay change.
    pub fn protocol_version(&self) -> amaru_kernel::ProtocolVersion {
        self.protocol_parameters().protocol_version
    }

    /// Obtain the latest protocol parameters: the cached base, overlaid with any in-flight change
    /// pending in the volatile overlay.
    pub fn protocol_parameters(&self) -> &ProtocolParameters {
        self.volatile.protocol_parameters()
    }

    /// Like `Self::protocol_parameters` but for a given epoch (must be reachable; i.e. current or
    /// previous)
    pub fn protocol_parameters_for(&self, epoch: Epoch) -> Option<&ProtocolParameters> {
        self.volatile.protocol_parameters_for(epoch)
    }

    /// Obtain the latest governance activity, folding in any pending dormant-epoch bump from the
    /// volatile overlay.
    pub fn governance_activity(&self) -> GovernanceActivity {
        self.volatile.governance_activity()
    }

    /// Like `Self::governance_activity` but for a given epoch (must be reachable; i.e. current or
    /// previous)
    pub fn governance_activity_for(&self, epoch: Epoch) -> Option<GovernanceActivity> {
        self.volatile.governance_activity_for(epoch)
    }
}

impl<S: Store, HS: HistoricalStores + Send> State<S, HS> {
    pub fn new(
        stable: S,
        snapshots: HS,
        network: NetworkName,
        era_history: EraHistory,
        global_parameters: GlobalParameters,
    ) -> Result<Self, StoreError> {
        let protocol_parameters = stable.protocol_parameters()?;

        protocol_version::validate(protocol_parameters.protocol_version, protocol_version::MINIMUM_SUPPORTED)
            .map_err(|e| StoreError::Internal(Box::new(e)))?;

        let governance_activity = stable.governance_activity()?;

        let stake_distributions = initial_stake_distributions(&snapshots, &era_history)?;

        let epoch = unsafe_slot_to_epoch(&era_history, stable.tip()?.slot_or_default());

        Ok(Self::new_with(
            stable,
            snapshots,
            epoch,
            network,
            era_history,
            global_parameters,
            protocol_parameters,
            governance_activity,
            stake_distributions,
        ))
    }

    #[expect(clippy::too_many_arguments)]
    pub fn new_with(
        stable: S,
        snapshots: HS,
        epoch: Epoch,
        network: NetworkName,
        era_history: EraHistory,
        global_parameters: GlobalParameters,
        protocol_parameters: ProtocolParameters,
        governance_activity: GovernanceActivity,
        stake_distributions: VecDeque<StakeDistribution>,
    ) -> Self {
        Self {
            stable: Arc::new(Mutex::new(stable)),

            snapshots,

            // NOTE: At this point, we always restart from an empty volatile state; which means
            // that there needs to be some form of synchronization between the consensus and the
            // ledger here. Few assumptions also stems from this:
            //
            // (1) The consensus must be storing its own state, and in particular, where it has
            //     left the synchronization.
            //
            // (2) Re-applying GlobalParameters::consensus_security_param (already synchronized) blocks is _fast-enough_ that it can be
            //     done on restart easily. To be measured; if this turns out to be too slow, we
            //     views of the volatile DB on-disk to be able to restore them quickly.
            volatile: VolatileDB::new(epoch, protocol_parameters, governance_activity),

            global_parameters: Arc::new(global_parameters),

            stake_distributions: Arc::new(Mutex::new(stake_distributions)),

            era_history: Arc::new(era_history),

            network,

            on_stake_dist_updated: None,
        }
    }

    /// Set a callback to be invoked when a new stake distribution snapshot becomes available.
    /// The callback receives the projected PoolSummaries.
    pub fn set_on_stake_dist_updated(&mut self, cb: Arc<dyn Fn(PoolSummaries) + Send + Sync>) {
        self.on_stake_dist_updated = Some(cb);
    }

    /// Project the small pool summaries needed for header validation (and leader schedule)
    /// from the held stake distributions. Only the `.pools` data is included.
    pub fn pool_summaries(&self) -> PoolSummaries {
        #[expect(clippy::unwrap_used)]
        let guard = self.stake_distributions.lock().unwrap();
        let mut by_epoch = BTreeMap::new();
        for distr in guard.iter() {
            let mut pools: BTreeMap<PoolId, PoolSummary> = BTreeMap::new();
            for (pid, pst) in &distr.pools {
                pools.insert(
                    *pid,
                    PoolSummary { vrf: pst.parameters.vrf, stake: pst.stake, active_stake: distr.active_stake },
                );
            }
            by_epoch.insert(distr.epoch, pools);
        }
        PoolSummaries { by_epoch }
    }

    pub fn network(&self) -> NetworkName {
        self.network
    }

    pub fn era_history(&self) -> &EraHistory {
        &self.era_history
    }

    pub fn global_parameters(&self) -> &GlobalParameters {
        &self.global_parameters
    }

    pub fn most_recent_snapshot(&self) -> Epoch {
        self.volatile.most_recent_snapshot(&self.snapshots)
    }

    /// Inspect the tip of this ledger state. This corresponds to the point of the latest block
    /// applied to the ledger.
    pub fn tip(&'_ self) -> Cow<'_, Point> {
        if let Some(st) = self.volatile.view_back() {
            return Cow::Owned(st.anchor.0.point());
        }

        Cow::Owned(self.immutable_tip())
    }

    #[expect(clippy::panic)]
    #[expect(clippy::unwrap_used)]
    /// Tip of the immutable db (i.e. farthest point we can ever rollback to).
    pub fn immutable_tip(&self) -> Point {
        self.stable.lock().unwrap().tip().unwrap_or_else(|e| panic!("no tip found in stable db: {e:?}"))
    }

    /// Tip of the volatile (`VolatileDB`) sequence only, if non-empty.
    pub fn volatile_tip(&self) -> Option<Tip> {
        self.volatile.view_back().map(|fragment| fragment.tip())
    }

    /// Get the registered relay socket addresses from the stable store.
    ///
    /// **NOTE:** This operation blocks the ledger for about 4ms (mainnet late
    /// 2025), so it should be called with care. Please cache the result, it
    /// only changes meaningfully once per epoch.
    #[expect(clippy::unwrap_used)]
    pub fn registered_relay_socket_addrs(&self) -> Result<BTreeSet<SocketAddr>, StateError> {
        let db = self.stable.lock().unwrap();
        Ok(crate::registered_relay_addrs::collect_from_read_store(&*db)?)
    }

    #[expect(clippy::unwrap_used)]
    fn apply_block(&mut self, now_stable: AnchoredVolatileFragment) -> Result<(), StateError> {
        let immutable_slot = now_stable.anchor.0.slot();
        let immutable_epoch = unsafe_slot_to_epoch(&self.era_history, immutable_slot);

        debug_span!(ledger::block::APPLY, point_slot = immutable_slot).in_scope(
            || {
                let protocol_parameters = self.protocol_parameters_for(immutable_epoch).unwrap_or_else(|| unreachable! {
                    "invariant violation: asking protocol parameters for an unreachable epoch; immutable epoch = {}; volatile epoch = {}",
                    immutable_epoch,
                    self.epoch(),
                });

                // Persist changes for this block
                let StoreUpdate { point: stable_point, issuer: stable_issuer, fees, donations, add, remove, withdrawals } =
                    now_stable.into_store_update(immutable_epoch, protocol_parameters);

                self.stable.lock().unwrap()
                    .with_transaction(|batch| {
                        batch.save(
                            &self.era_history,
                            protocol_parameters,
                            self.governance_activity_for(immutable_epoch).unwrap_or_else(|| unreachable! {
                                "invariant violation: asking governance activity for an unreachable epoch; immutable epoch = {}; volatile epoch = {}",
                                immutable_epoch,
                                self.epoch(),
                            }),
                            &stable_point,
                            Some(&stable_issuer),
                            add,
                            remove,
                            withdrawals,
                        )?;

                        batch.with_pots(|mut row| {
                            let row = row.borrow_mut();
                            row.fees += fees;
                            row.donations += donations;
                        })?;

                        batch.reset_epoch_transition_progress()?;

                        Ok(())
                    })
                    .map_err(StateError::Storage)?;

               Ok(())
            },
        )
    }

    /// Check whether the next state should cause an epoch transition. This is the case when it
    /// corresponds to a block in a different (next) epoch, in which case, we must first transition
    /// into the new epoch before the block can be validated.
    fn try_epoch_transition(&mut self, next_tip: Point) -> Result<(), StateError> {
        let current_epoch = unsafe_slot_to_epoch(&self.era_history, self.tip().slot_or_default());
        let next_epoch = unsafe_slot_to_epoch(&self.era_history, next_tip.slot_or_default());

        if next_epoch > current_epoch {
            let old_protocol_version = self.protocol_version();

            self.epoch_transition(next_epoch)?;

            let new_protocol_version = self.protocol_version();

            if old_protocol_version != new_protocol_version {
                info!(
                    ledger::protocol::UPGRADE,
                    old_version = old_protocol_version.0,
                    new_version = new_protocol_version.0
                );
            }
        }

        Ok(())
    }

    fn epoch_transition(&mut self, next_epoch: Epoch) -> Result<(), StateError> {
        info_span!(ledger::epoch_transition::COMPUTE, from = next_epoch - 1, into = next_epoch).in_scope(|| {
            let computed_rewards = self.volatile.take_computed_rewards();

            #[allow(clippy::unwrap_used)]
            let db = self.stable.lock().unwrap();

            let progress = db.epoch_transition_progress()?;

            match progress {
                Some(resuming_from) => {
                    Span::current().record("resuming_from", resuming_from.to_string());
                }
                // NOTE: Skipping epoch transition
                //
                // It is possible to interrupt Amaru just after the epoch transition was flushed
                // to disk. The consequence of that is: the tip of the immutable db is still in the
                // previous epoch which will cause the next block we see to trigger an epoch transition.
                //
                // However, the epoch transition had already happened and was even persisted to disk
                // already! So we must not redo it. This strange behaviour occurs because we do not
                // persist the volatile; so on restart, we rewind `k` blocks in the past, for which we
                // may or may not need to perform the transition again (depending where we interrupted).
                None if self.most_recent_snapshot() == next_epoch - 1 => {
                    Span::current().record("skipped", true);
                    self.volatile.transition_already_persisted(next_epoch);
                    return Ok(());
                }
                None => (),
            }

            // NOTE: Crossing states during epoch transition
            //
            // The volatile at this point MUST NOT contain any block applications belonging to
            // two epochs; So it is crucical for this view to only be created before we introduce
            // any block from the next epoch.
            //
            // We could possible replace the direct access on the volatile here with an
            // aggregated state as a proof that the volatile was indeed only containing the
            // last k blocks for a single epoch. Or carry some kind of type-level guard that
            // the this is called within an acceptable context (i.e. the volatile
            // pre-conditions have been checked).
            let mut volatile_view = VolatileView::new(&self.volatile, &*db);

            // NOTE: No rewards during epoch transition?
            //
            // It is fine in some situation to compute an epoch transition and yet have no rewards.
            // This happens if Amaru is interrupted *while it is flushing* an epoch transition to
            // disk.
            //
            // This happens artificially every time someone bootstraps; because the bootstrapping
            // process behaves as if we had interrupted the transition just after taking the
            // snapshot. So we must proceed with computing the beginning of an epoch (ratification,
            // pool updates, etc...) but not the end (rewards).
            let (treasury, effective_rewards) = if progress.is_none() {
                let effective_rewards = Rewards::<Effective>::new(
                    // FIXME: asynchronous rewards calculations
                    //
                    // This should eventually be a '.await', as we always expect to *eventually*
                    // have some rewards summary being available. There's no way to continue progressing
                    // the ledger if we don't.
                    computed_rewards.ok_or(StateError::RewardsSummaryNotReady)?,
                    volatile_view.iter_unregistered_accounts()?.collect(),
                );

                (db.pots()?.treasury + effective_rewards.delta_treasury(), Some(effective_rewards))
            } else {
                (db.pots()?.treasury, None)
            };

            let protocol_parameters = self.protocol_parameters();

            // Compute the updates to perform on pools at the epoch boundary. This uses information
            // from both the immutable store and the volatile database, since we compute the updates
            // before they are "stable" and safe to store.
            let pools_updates = PoolsEpochTransitionUpdates::new(volatile_view.iter_pools()?, next_epoch);

            let ratification_context = RatificationContext::new(
                // Ratification happens with one epoch of delay, and at the next epoch transition. So,
                // if we ratify votes that happened in epoch `e`, the ratification is done during the
                // transition from `e + 1` to `e + 2`;
                //
                // Here, we have `next_epoch = e + 2`. And so, we have to pull the data and stake
                // distribution from at `next_epoch - 2`.
                self.snapshots.for_epoch(next_epoch - 2)?,
                self.stake_distribution(next_epoch - 2)?,
                protocol_parameters.clone(),
                // NOTE: ratification treasury value
                //
                // Ratification occurs after rewards have been paid out; and thus, uses the value
                // of the treasury that already includes any unpaid rewards.
                treasury,
            )?;

            // Ratify and enact proposals at the epoch boundary. Note that this does not modify the
            // immutable store in any fashion (db is read-only here) but produces a series of
            // governance updates to be applied to the database once stable; and use in-memory in the
            // meantime.
            let governance_updates = GovernanceUpdates::new(
                volatile_view.proposals_roots()?,
                volatile_view.iter_proposals()?,
                &self.era_history,
                protocol_parameters,
                ratification_context,
            )?;

            drop(db); // Dropping the *mutable reference*, not the *actual database* :)

            self.volatile.transition(effective_rewards, pools_updates, governance_updates);

            Ok(())
        })
    }

    fn try_compute_rewards(&mut self) -> Result<(), StateError> {
        let tip = self.tip().slot_or_default();
        let current_epoch = unsafe_slot_to_epoch(&self.era_history, tip);
        let is_previous_epoch_stable =
            self.era_history.slot_in_epoch(tip, tip).unwrap_or_default() >= self.global_parameters().stability_window();

        // FIXME: Asynchronous rewards calculation
        //
        // compute rewards in a thread, or in a non-blocking manner to carry on with other
        // tasks while rewards are being computed; they only need to be available at the epoch
        // boundary.
        if self.volatile.rewards_not_ready()
            && self.most_recent_snapshot() == current_epoch - 1
            && is_previous_epoch_stable
        {
            let (computed, pushed_new) = self.compute_rewards(current_epoch)?;
            self.volatile.set_computed_rewards(computed);
            if pushed_new && let Some(cb) = &self.on_stake_dist_updated {
                cb(self.pool_summaries());
            }
        }

        Ok(())
    }

    #[expect(clippy::unwrap_used)]
    fn compute_rewards(&mut self, for_epoch: Epoch) -> Result<(RewardsSummary, bool), StateError> {
        let span = info_span!(ledger::rewards::COMPUTE, for_epoch = for_epoch);

        // NOTE: Explicit span guard handling
        //
        // We resort to manually entering and leaving the span here to avoid measuring the
        // 'compute_stake_distribution' as part of the 'compute_rewards' but instead, have each in
        // a separate span.
        //
        // The reason they happen in the same function here is because they both modify the
        // shared 'stake_distributions' that lives behind a mutex. So to avoid holding the mutext
        // for too long, we resort to that trick.
        let span_guard = span.enter();

        let mut stake_distributions = self.stake_distributions.lock().unwrap();
        let stake_distribution =
            stake_distributions.pop_back().ok_or(StateError::StakeDistributionNotAvailableForRewards)?;

        assert_eq!(stake_distribution.epoch + 3, for_epoch, "unexpected stake distribution for epoch");

        span.record("using_stake_distribution_from", u64::from(stake_distribution.epoch));
        let snapshot = self.snapshots.for_epoch(for_epoch - 1)?;

        let rewards_summary =
            RewardsSummary::new(&snapshot, stake_distribution, &self.global_parameters, self.protocol_parameters())
                .map_err(StateError::Storage)?;
        drop(span_guard);

        let mut pushed_new = false;

        if stake_distributions.front().map(|distr| distr.epoch < snapshot.epoch()).unwrap_or(true) {
            stake_distributions.push_front(compute_stake_distribution(&snapshot, &self.era_history)?);
            pushed_new = true;
            info!(
                ledger::stake_distribution::ROTATE,
                available_stake_distributions = display_collection(stake_distributions.iter().map(|distr| distr.epoch)),
            );
        }

        Ok((rewards_summary, pushed_new))
    }

    /// Push a next state into the ledger volatile storage. Once the volatile is full (i.e. filled
    /// with `k` state updates); a push will yield a stable state to apply. Otherwise, this simply
    /// fills the volatile.
    pub fn push_fragment(
        &mut self,
        state: AnchoredVolatileFragment,
    ) -> Result<Option<AnchoredVolatileFragment>, StateError> {
        debug_span!(ledger::state::PUSH).in_scope(|| {
            let security_param = self.global_parameters.consensus_security_param;

            // Yield any now-stable state change
            let now_stable = if self.volatile.len() as u64 >= security_param {
                let now_stable = self.volatile.pop_front().unwrap_or_else(|| {
                    unreachable!(
                        "pre-condition: self.volatile.len()={} >= consensus_security_param={}",
                        self.volatile.len(),
                        self.global_parameters.consensus_security_param
                    )
                });

                Some(now_stable)
            } else {
                trace!(ledger::volatile::WARM_UP, size = self.volatile.len());
                None
            };

            self.volatile.push_back(state);

            Ok(now_stable)
        })
    }

    #[allow(clippy::unwrap_used)]
    fn apply_transition(&mut self) -> Result<(), StateError> {
        if self.volatile.is_epoch_transition_stable(self.era_history(), self.global_parameters()) {
            if let Some((len, epoch_tail)) = self.volatile.epoch_tail() {
                // NOTE: Forcing snapshot after 3*k/f slots
                //
                // In a healthy chain that honors the Chain Growth property, we should never reach
                // this line; because we would have seen `k` blocks before that point and there
                // should be no trailing epoch tail in the volatile. However, Cardano does not
                // necessarily honors Chain Growth; and we can detect this as such.
                //
                // Yet, we must produce a new snapshot in order to produce a new stake distribution
                // to keep validating upcoming block headers.
                warn!(
                    ledger::chain_growth::VIOLATE,
                    unstable_tail_length = len,
                    reason = format!(
                        "chain growth violation: less than k={k} blocks seen in a window of \
                         3*k/f={stability_window} slots; if this occurs during historical sync, it may \
                         not be a big problem. However If this occurs at the tip, it can be more \
                         serious. We will not be able to rollback through still-unstable blocks that \
                         must now be persisted to disk.",
                        k = self.global_parameters().consensus_security_param,
                        stability_window = self.global_parameters().stability_window()
                    )
                );

                for anchored_fragment in epoch_tail {
                    self.apply_block(anchored_fragment)?;
                }
            }

            let db = self.stable.lock().unwrap();

            self.volatile.apply_transition(&*db)?;
            self.snapshots.prune(self.volatile.epoch() - MIN_LEDGER_SNAPSHOTS)?;
        }

        Ok(())
    }

    /// View a stake distribution for a given epoch. Note that this *locks* the stake distribution
    /// mutext, meaning that it might block other thread awaiting to acquire this data.
    ///
    /// So this shall be used when the data is needed for a short time, and one doesn't want to
    /// the full mutex around.
    fn stake_distribution(&self, epoch: Epoch) -> Result<StakeDistributionView<'_>, StateError> {
        let guard = self.stake_distributions.lock().map_err(|_| StateError::FailedToAcquireStakeDistrLock)?;
        StakeDistributionView::new(guard, epoch)
    }

    /// Create a validation context for a whole block.
    #[allow(clippy::unwrap_used)]
    fn create_block_validation_context(&self, block: &Block) -> Result<DefaultValidationContext, StateError> {
        debug_span!(
            ledger::block_validation_context::CREATE,
            block_body_hash = block.header.header_body.block_body_hash,
            block_number = block.header.header_body.block_number,
            block_body_size = block.header.header_body.block_body_size
        )
        .in_scope(|| {
            let mut ctx = DefaultPreparationContext::new();
            rules::prepare_block(&mut ctx, block);
            let db = &*self.stable.lock().unwrap();
            ctx.into_validation_context(
                UnresolvedInputPolicy::Defer,
                // FIXME: Delayed proposal roots
                //
                // The Volatile's proposal_roots currently return an Option, but it really should
                // return a plain object and be tracking the live candidate root (which may be
                // updated by every fragment).
                //
                // Without that, we cannot properly validate proposals that depend on other
                // proposals that have been submitted in the same epoch; since our roots are only
                // updated on every epoch boundary. This means that we must properly initialize the
                // roots on startup, from the "stable" roots (last changed after last
                // ratification), and all the currently pending proposals.
                self.volatile.proposals_roots().cloned().unwrap_or(db.proposals_roots().map_err(StateError::Storage)?),
                &self.volatile,
                db,
            )
            .map_err(StateError::ContextHydratation)
        })
    }

    /// Create a validation context for a single transaction.
    #[allow(clippy::unwrap_used)]
    fn create_transaction_validation_context(
        &self,
        transaction: &Transaction,
    ) -> Result<DefaultValidationContext, StateError> {
        let transaction_id = transaction.tx_id();
        debug_span!(ledger::transaction_validation_context::CREATE, transaction_id = transaction_id).in_scope(|| {
            let mut ctx = DefaultPreparationContext::new();
            rules::prepare_transaction(&mut ctx, &transaction.body);
            let db = &*self.stable.lock().unwrap();
            ctx.into_validation_context(
                UnresolvedInputPolicy::Reject,
                self.volatile.proposals_roots().cloned().unwrap_or(db.proposals_roots().map_err(StateError::Storage)?),
                &self.volatile,
                db,
            )
            .map_err(StateError::ContextHydratation)
        })
    }

    /// Create a validation context from the current ledger state for the transaction, and
    /// validate the transaction against it.
    ///
    /// Note that the transaction pointer is provided in order to pass an estimate of what would be
    /// the slot for that transaction since some ledger rules require the slot.
    /// The `transaction_index` is irrelevant for mempool transactions so it's left to 0.
    pub fn validate_tx(
        &self,
        transaction: &Transaction,
        slot: Slot,
        arena_pool: &ArenaPool,
    ) -> Result<(), TransactionValidationError> {
        let transaction_id = transaction.tx_id();

        let mut context = self
            .create_transaction_validation_context(transaction)
            .map_err(|error| TransactionValidationError::Preparation { transaction_id, error })?;

        let tx_size = to_cbor(transaction).len() as u64;

        rules::block::validate_transaction(
            &mut context,
            arena_pool,
            self.network(),
            self.protocol_parameters(),
            self.era_history(),
            self.global_parameters(),
            self.governance_activity(),
            TransactionPointer { slot, transaction_index: 0 },
            transaction,
            tx_size,
        )
        .map_err(|violation| TransactionValidationError::Validation { transaction_id, violation: Box::new(violation) })
    }

    /// Roll the ledger forward given a new upcoming block. This roughly unwinds the following
    /// steps:
    ///
    /// 1. **Rewards Calculations**
    ///
    ///    Begin the rewards calculation if we are now within the stability window (3 * k / f slots
    ///    deep in the epoch).
    ///
    /// 2. **Epoch Transition**
    ///
    ///    Try to transition into a new epoch should the block make the ledger cross an epoch
    ///
    /// 3. **Validation Context**
    ///
    ///    Create a validation context from the current stable ledger state + epoch transition if any
    ///
    /// 4. **Ledger rules execution**
    ///
    ///    Runs validation rules, collecting and aggregating block updates into a single update
    ///    fragment.
    ///
    /// 5. **Record new volatile fragment**
    ///
    ///    Anchor those updates and push them into the volatile store.
    ///
    /// 6. **Apply now-stable block**
    ///
    ///    Finally, we can store the new now-stable block to the stable store.
    ///
    /// 7. **Flush epoch transition**
    ///
    ///    In normal operations (i.e. once the ledger is done warming up), pushing a new state to
    ///    the volatile automatically yields a new now-stable state that is recorded to disk.
    ///
    ///    Before attempting to record the next block from a new epoch to disk, any pending epoch
    ///    transition must be fully flushed and a snapshot taken.
    pub fn roll_forward(
        &mut self,
        point: &Point,
        block: Block,
        arena_pool: &ArenaPool,
    ) -> BlockValidation<LedgerMetrics, anyhow::Error> {
        debug_span!(ledger::state::ROLL_FORWARD).in_scope(|| {
            let block_height = block.header.header_body.block_number;

            trace_block_transactions(point, block_height, &block);

            // 1. Rewards calculation
            BlockValidation::from(self.try_compute_rewards())?;

            // 2. Epoch transition
            BlockValidation::from(self.try_epoch_transition(*point))?;

            let issuer = Hasher::<224>::hash(&block.header.header_body.issuer_vkey[..]);

            let metrics = self.new_metrics(point, &block, issuer);

            // 3. Validation context
            let mut context = BlockValidation::from(self.create_block_validation_context(&block))?;

            // 4. Ledger rules execution
            rules::validate_block(
                &mut context,
                arena_pool,
                self.network(),
                self.protocol_parameters(),
                self.era_history(),
                self.global_parameters(),
                self.governance_activity(),
                block,
            )?;

            // 5. Record new volatile state
            let tip = Tip::new(*point, block_height.into());
            let fragment = VolatileFragment::from(context).anchor(tip, issuer);
            if let Some(now_stable) = BlockValidation::from(self.push_fragment(fragment))? {
                // 6. Apply now-stable block
                BlockValidation::from(self.apply_block(now_stable))?;
            }

            // 7. Flush the epoch transition
            BlockValidation::from(self.apply_transition())?;

            BlockValidation::Valid(metrics)
        })
    }

    fn new_metrics(&self, point: &Point, block: &Block, issuer: Hash<28>) -> LedgerMetrics {
        let slot = point.slot_or_default();

        let prev_hash = block.header.header_body.prev_hash;

        let block_height = block.header.header_body.block_number;

        let epoch = self
            .era_history()
            .slot_to_epoch(slot, slot)
            .unwrap_or_else(|e| unreachable!("impossible; failed to compute epoch from current slot ({slot}): {e}"));

        let slot_in_epoch = self.era_history().slot_in_epoch(slot, slot).unwrap_or_else(|e| {
            unreachable!("impossible; failed to compute relative slot from current slot ({slot}): {e}")
        });

        let density = self.chain_density(point);

        let current_kes_period = u64::from(slot).checked_div(self.global_parameters.slots_per_kes_period).unwrap_or(0);

        let remaining_kes_periods =
            (self.global_parameters.max_kes_evolution as u64).saturating_sub(current_kes_period);

        LedgerMetrics {
            block_height,
            slot: u64::from(slot),
            slot_in_epoch: u64::from(slot_in_epoch),
            epoch: u64::from(epoch),
            density,
            current_kes_period,
            remaining_kes_periods,
            block_header_hash: hex::encode(point.hash()),
            parent_block_header_hash: prev_hash.map(hex::encode).unwrap_or_default(),
            issuer_verification_key_hash: hex::encode(issuer),
        }
    }

    /// Try to rollback the volatile state to a given point and roll forward a number of block by applying
    /// them after the fork point. Recover the initial state in case of errors.
    pub fn switch_to_fork<I>(
        &mut self,
        fork_point: &Point,
        blocks: I,
        arena_pool: &ArenaPool,
    ) -> BlockValidation<LedgerMetrics, anyhow::Error>
    where
        I: IntoIterator<Item = anyhow::Result<(Point, Block)>>,
        I::IntoIter: ExactSizeIterator,
    {
        let blocks = blocks.into_iter();
        let count = blocks.len();

        info_span!(ledger::state::SWITCH_TO_FORK, fork_point = *fork_point, fork_length = count).in_scope(|| {
            let recover = match self.rollback_to(fork_point) {
                Ok(state_recovery) => move |st: &mut Self| {
                    let immutable_tip = st.immutable_tip();

                    assert_eq!(
                        immutable_tip, state_recovery.immutable_tip,
                        "cannot recover: immutable tip moved from {} to {} during the replay",
                        state_recovery.immutable_tip, immutable_tip,
                    );

                    match state_recovery.kind {
                        StateRecovery::RecoverWholeVolatileDB { volatile } => {
                            st.volatile = *volatile;
                        }
                        StateRecovery::RecoverVolatileDBPart { recovery } => {
                            st.volatile.undo_rollback(*recovery);
                        }
                    }
                },

                Err(error) => return BlockValidation::Err(error.into()),
            };

            self.assert_replay_stays_volatile(count);

            let mut metrics = LedgerMetrics::default();

            for block in blocks {
                let (point, block) = match block {
                    Ok(block) => block,
                    Err(error) => {
                        recover(self);
                        return BlockValidation::Err(error);
                    }
                };
                match self.roll_forward(&point, block, arena_pool) {
                    BlockValidation::Valid(new_metrics) => metrics = new_metrics,
                    BlockValidation::Invalid(slot, hash, details) => {
                        recover(self);
                        return BlockValidation::Invalid(slot, hash, details);
                    }
                    BlockValidation::Err(error) => {
                        recover(self);
                        return BlockValidation::Err(error);
                    }
                }
            }

            BlockValidation::Valid(metrics)
        })
    }

    /// Assert, before replaying a fork, that the replay cannot flush anything to the stable store.
    ///
    /// Called with the number of blocks about to be replayed, right after the rollback. Replaying
    /// evicts a block to the stable store only once the volatile window is full (see
    /// [`Self::push_fragment`]); with `blocks` blocks to apply, the earliest such eviction can only
    /// land on the *last* block as long as `volatile.len() + blocks - 1 <= k`. That is exactly the
    /// case where the new chain is at most one block longer than the one it replaces. The committing
    /// block may then legitimately become stable, but every earlier block stays fully volatile. So
    /// if a later block turns out to be invalid, [`Self::recover`] can always undo the switch without
    /// having to un-persist immutable data.
    fn assert_replay_stays_volatile(&self, blocks: usize) {
        let capacity = self.global_parameters.consensus_security_param;
        let non_committing = self.volatile.len() as u64 + blocks.saturating_sub(1) as u64;
        assert!(
            non_committing <= capacity,
            "fork-switch replay would flush a still-rollback-able block to the stable store: after \
             rollback the volatile holds {} block(s) and replaying {} would push {} past the \
             security parameter k={} before reaching the committing block",
            self.volatile.len(),
            blocks,
            non_committing,
            capacity,
        );
    }

    fn rollback_to<'a>(&mut self, to: &'a Point) -> Result<RollbackGuard<'a>, BackwardError> {
        info_span!(ledger::state::ROLL_BACKWARD).in_scope(|| {
            let immutable_tip = self.immutable_tip();
            let volatile_tip = self.volatile_tip().map(|t| t.point()).unwrap_or(immutable_tip);

            // NOTE: Rolling back to the tip of the immutable
            //
            // All rollback points within the volatile part are handled by `VolatileDB`, but there is one more
            // legal rollback target, which is the `immutable_tip()`, in which case the VolatileDB is cleared.
            if *to == immutable_tip {
                // Snapshot the whole VolatileDB fragment but leave the metadata initialized
                // for the upcoming roll forwards.
                Ok(RollbackGuard {
                    immutable_tip,
                    kind: StateRecovery::RecoverWholeVolatileDB { volatile: Box::new(self.volatile.clear()) },
                })
            } else if *to < immutable_tip {
                Err(BackwardError::beyond_max(*to, volatile_tip, immutable_tip))
            } else if *to > volatile_tip {
                Err(BackwardError::in_the_future(*to, volatile_tip, immutable_tip))
            } else {
                // Rollback to the fork point and keep the recovery instance in case
                // a subsequent roll forward fails to apply and we need to recover the previous
                // ledger state.
                let recovery = self
                    .volatile
                    .rollback_to(to)
                    .map_err(|_| BackwardError::unknown(*to, volatile_tip, immutable_tip))?;
                Ok(RollbackGuard {
                    immutable_tip,
                    kind: StateRecovery::RecoverVolatileDBPart { recovery: Box::new(recovery) },
                })
            }
        })
    }

    /// Calculate chain density over the last `k` blocks (or oldest block in the volatileDB) given some `Point`.
    /// If the `Point` is older than the oldest block in the volatileDB, density is 0
    pub fn chain_density(&self, point: &Point) -> f64 {
        let latest_slot = point.slot_or_default();
        let k_slot =
            self.volatile.view_front().map(|anchored| anchored.point()).unwrap_or(Point::Origin).slot_or_default();

        if k_slot >= latest_slot {
            0f64
        } else {
            max(1, self.volatile.len()) as f64 / (u64::from(latest_slot) - u64::from(k_slot)) as f64
        }
    }
}

// NOTE: Initialize stake distribution held in-memory. The one before last is needed by the
// consensus layer to validate the leader schedule, while the one before that will be
// consumed for the rewards calculation.
//
// We always hold on two stake distributions:
//
// - The one from an epoch `e - 1` which is used for the ongoing leader schedule at epoch `e + 1`
// - The one from an epoch `e - 2` which is used for the rewards calculations at epoch `e + 1`
//
// Note that the most recent snapshot we have is necessarily `e`, since `e + 1` designates
// the ongoing epoch, not yet finished (and so, not available as snapshot).
pub fn initial_stake_distributions<HS>(
    snapshots: &HS,
    era_history: &EraHistory,
) -> Result<VecDeque<StakeDistribution>, StoreError>
where
    HS: HistoricalStores + Send,
{
    use rayon::prelude::*;

    let latest_epoch = snapshots.most_recent_snapshot();
    let epoch_for_leader_schedule = latest_epoch.checked_sub(Epoch::ONE);
    let epoch_for_rewards = latest_epoch.checked_sub(Epoch::TWO);

    [Some(latest_epoch), epoch_for_leader_schedule, epoch_for_rewards]
        .into_iter()
        .filter_map(|epoch| epoch.map(|e| snapshots.for_epoch(e)))
        .collect::<Result<Vec<_>, _>>()?
        .into_par_iter()
        .map(|snapshot| compute_stake_distribution(&snapshot, era_history))
        .collect::<Result<VecDeque<_>, _>>()
        .map_err(|err| StoreError::Internal(err.into()))
}

pub fn compute_stake_distribution(
    snapshot: &impl Snapshot,
    era_history: &EraHistory,
) -> Result<StakeDistribution, StateError> {
    info_span!(ledger::stake_distribution::COMPUTE, epoch = snapshot.epoch(),).in_scope(|| {
        StakeDistribution::new(snapshot, GovernanceSummary::new(snapshot, era_history)?).map_err(StateError::Storage)
    })
}

// StakeDistributionView
// ----------------------------------------------------------------------------

/// A object to carry a locked view on a stake distribution of a specific epoch. The lock is
/// dropped as soon as the viewer goes out of scope.
pub struct StakeDistributionView<'a> {
    guard: MutexGuard<'a, VecDeque<StakeDistribution>>,
    position: usize,
}

impl<'a> StakeDistributionView<'a> {
    pub fn new(guard: MutexGuard<'a, VecDeque<StakeDistribution>>, epoch: Epoch) -> Result<Self, StateError> {
        let position = guard
            .iter()
            .position(|distr| distr.epoch == epoch)
            .ok_or(StateError::NoSuitableStakeDistribution(epoch))?;

        Ok(Self { guard, position })
    }
}

impl<'a> Deref for StakeDistributionView<'a> {
    type Target = StakeDistribution;
    fn deref(&self) -> &Self::Target {
        // Safe, because Self can only be created after checking that the index was present. Plus,
        // we hold the guard, so that data cannot change.
        &self.guard[self.position]
    }
}

fn trace_block_transactions(point: &Point, block_height: u64, block: &Block) {
    let tx_count = block.transaction_bodies.len();

    trace!(ledger::non_empty_block::FOUND, %point, block_height, tx_count);

    if !tracing_enabled!(tracing::Level::TRACE) {
        return;
    }

    for (tx_index, body) in block.transaction_bodies.iter().enumerate() {
        let tx_id = body.tx_id();
        trace!(ledger::transaction::FOUND, %point, block_height, tx_index, tx_id = %tx_id);
    }
}

// NOTE: calculating current epoch from slot on block application.
//
// This is only safe provided the next_tip is within the foreseeable window. If this isn't
// the case, it's a clear signal of something going very wrong in the consensus/networking
// pipeline feeding blocks to the ledger since they'd be attempting to feed a block that is
// many day after the last applied block!
fn unsafe_slot_to_epoch(era_history: &EraHistory, slot: Slot) -> Epoch {
    era_history
        .slot_to_epoch_unchecked_horizon(slot)
        .unwrap_or_else(|e| unreachable!("impossible; failed to compute epoch from tip ({slot:?}): {e:?}"))
}

// Rollback
// ----------------------------------------------------------------------------

/// Captures what a rollback discards, so a failed fork switch can be undone.
/// If the fork point is inside the volatile window, we keep only the fragments above that point (moved, not copied)
/// plus a snapshot of the volatile overlay.
///
/// The immutable tip observed at rollback time is retained so recovery can assert it has not moved:
/// restoring the pre-rollback volatile is only sound while no replayed block has reached the stable store.
#[derive(Debug)]
struct RollbackGuard<'a> {
    immutable_tip: Point,
    kind: StateRecovery<'a>,
}

#[derive(Debug)]
enum StateRecovery<'a> {
    /// A rollback to the immutable tip cleared the whole window; the entire pre-rollback volatile
    /// is moved out (via [`VolatileDB::take`]) and restored wholesale.
    RecoverWholeVolatileDB { volatile: Box<VolatileDB> },
    /// A rollback within the volatile window; only the discarded parts are captured.
    RecoverVolatileDBPart { recovery: Box<volatile::RollbackGuard<'a>> },
}

// Errors
// ----------------------------------------------------------------------------

/// The ledger has been instructed to rollback to an unknown point. These should be impossible
/// if chain-sync messages (roll-forward and roll-backward) are all passed to the ledger.
#[derive(Debug, Error)]
pub enum BackwardError {
    #[error("error rolling back to unknown point: {0}")]
    UnknownRollbackPoint(BackwardErrorDetails),

    #[error("attempted to rollback beyond immutable tip: {0}")]
    BeyondMaxRollback(BackwardErrorDetails),

    #[error("attempted roll back in the future: {0}")]
    RollbackPointInFuture(BackwardErrorDetails),
}

impl BackwardError {
    pub fn rollback_point(&self) -> Point {
        match self {
            Self::UnknownRollbackPoint(BackwardErrorDetails { rollback_point, .. })
            | Self::BeyondMaxRollback(BackwardErrorDetails { rollback_point, .. })
            | Self::RollbackPointInFuture(BackwardErrorDetails { rollback_point, .. }) => **rollback_point,
        }
    }

    pub fn unknown(rollback_point: Point, volatile_tip: Point, immutable_tip: Point) -> Self {
        Self::UnknownRollbackPoint(BackwardErrorDetails::new(rollback_point, volatile_tip, immutable_tip))
    }

    pub fn beyond_max(rollback_point: Point, volatile_tip: Point, immutable_tip: Point) -> Self {
        Self::BeyondMaxRollback(BackwardErrorDetails::new(rollback_point, volatile_tip, immutable_tip))
    }

    pub fn in_the_future(rollback_point: Point, volatile_tip: Point, immutable_tip: Point) -> Self {
        Self::RollbackPointInFuture(BackwardErrorDetails::new(rollback_point, volatile_tip, immutable_tip))
    }
}

#[derive(Debug, Error)]
#[error("rollback point = {rollback_point}, volatile tip = {volatile_tip}, immutable_tip = {immutable_tip}")]
pub struct BackwardErrorDetails {
    rollback_point: Box<Point>,
    volatile_tip: Box<Point>,
    immutable_tip: Box<Point>,
}

impl BackwardErrorDetails {
    pub fn new(rollback_point: Point, volatile_tip: Point, immutable_tip: Point) -> Self {
        BackwardErrorDetails {
            rollback_point: Box::new(rollback_point),
            volatile_tip: Box::new(volatile_tip),
            immutable_tip: Box::new(immutable_tip),
        }
    }
}

#[derive(Debug, Error)]
pub enum StateError {
    #[error("error accessing storage: {0}")]
    Storage(#[from] StoreError),

    #[error("no stake distribution available for rewards calculation.")]
    StakeDistributionNotAvailableForRewards,

    #[error("failed to acquire stake distribution shared lock")]
    FailedToAcquireStakeDistrLock,

    #[error("no suitable stake distribution for requested epoch: {0}")]
    NoSuitableStakeDistribution(Epoch),

    // TODO: Using a mere 'String' here because the source error contains some `Rc`, which aren't
    // safe to send across threads. For the sake of carrying the error around, we might want to not
    // keep Rc in errors, but clone the underlying data -- which is small anyway, in places where
    // the error is generated.
    #[error("error when ratifying proposals: {0}")]
    RatificationFailed(String),

    #[error("rewards summary not ready")]
    RewardsSummaryNotReady,

    #[error("expected effective rewards to apply but found something else")]
    NoEffectiveRewards,

    #[error("failed to compute epoch from slot {0:?}: {1}")]
    ErrorComputingEpoch(Slot, EraHistoryError),

    #[error("failed to hydrate validation context")]
    ContextHydratation(#[source] ContextHydratationError),
}

#[derive(Debug, Error)]
pub enum TransactionValidationError {
    #[error("transaction {transaction_id} is invalid")]
    Validation {
        transaction_id: TransactionId,
        #[source]
        violation: Box<TransactionInvalid>,
    },
    #[error("failed to prepare transaction {transaction_id} for validation")]
    Preparation {
        transaction_id: TransactionId,
        #[source]
        error: StateError,
    },
}

impl From<governance::Error> for StateError {
    fn from(origin: governance::Error) -> Self {
        match origin {
            governance::Error::EraHistoryError(slot, err) => StateError::ErrorComputingEpoch(slot, err),
            governance::Error::StoreError(err) => StateError::Storage(err),
        }
    }
}
