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
    collections::{BTreeSet, VecDeque},
    sync::Arc,
    time::Instant,
};

use amaru_kernel::{
    Block, Epoch, EraHistory, EraHistoryError, GlobalParameters, Hash, Hasher, IsHeader, NetworkName, Point, PoolId,
    ProtocolParameters, Slot, Transaction, TransactionId, TransactionPointer,
    cardano::era_history::unsafe_slot_to_epoch, protocol_version, size::SCRIPT,
};
use amaru_metrics::ledger::LedgerMetrics;
use amaru_observability::{
    debug_span, error_record, info, info_record, info_span, trace, trace_record, warn, warn_record,
};
use amaru_ouroboros_traits::{ForkSwitchOutcome, InvalidBlock, PoolSummaries, ValidationMode};
use amaru_uplc::arena_pool::ArenaPool;
use thiserror::Error;
use tracing::Span;

use crate::{
    context::{ContextHydratationError, DefaultPreparationContext, DefaultValidationContext, UnresolvedInputPolicy},
    epoch_transition::{
        Computed, Effective, GovernanceActivity, GovernanceUpdates, PoolsEpochTransitionUpdates, Rewards,
    },
    governance::ratification::RatificationContext,
    observers::LedgerObservers,
    rules::{
        self,
        block::{BlockValidation, TransactionInvalid},
    },
    startup::{StartupContext, StartupHook},
    state::volatile::{
        AnchoredVolatileFragment, StoreUpdate, VolatileDB, VolatileFragment, VolatileMempool, VolatileSequence,
        VolatileView,
    },
    store::{HistoricalStores, Store, StoreError, TransactionalContext},
    summary::{
        governance::{self},
        stake_distribution::StakeDistribution,
    },
};

mod latest_snapshot;
pub mod tasks;
mod tip_update_emitter;
pub mod volatile;

use self::{
    latest_snapshot::{BeforeRollback, LatestSnapshot},
    tasks::compute_stake_distribution::initialize_stake_distributions,
    tip_update_emitter::TipUpdateEmitter,
};

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
///
/// Dropping the state cancels and joins the rewards worker before closing its stores.
pub struct State<S, HS>
where
    S: Store,
    HS: HistoricalStores,
{
    /// The stable store, exclusively owned by this ledger state.
    stable: S,

    /// Access to the per-epoch ledger snapshots.
    snapshots: HS,

    /// Our own in-memory vector of volatile deltas to apply onto the stable store in due time.
    volatile: VolatileDB,

    /// Global (i.e. non-updatable) parameters of the network. This includes things like
    /// slot length, epoch length, security parameter and other pieces that cannot generally
    /// be updated but grouped here to avoid dealing with magic values everywhere.
    global_parameters: Arc<GlobalParameters>,

    /// The era history for the network this store is related to.
    era_history: Arc<EraHistory>,

    /// Which network are we connected to. This is mostly helpful for distinguishing between
    /// behavious that are network specifics (e.g. address discriminant).
    network: NetworkName,

    /// Optional embedder observers (adopted blocks, full ledger stake summaries).
    observers: LedgerObservers,

    /// The most recent snapshot, final or candidate. A candidate snapshot is taken when the ledger
    /// needs the snapshot of the closing epoch before its last blocks are stable. See
    /// [`Store::create_candidate_snapshot`].
    latest_snapshot: LatestSnapshot,

    /// A local debounced tip emitter avoid flooding logs with tip updates during sync
    tip_update_emitter: TipUpdateEmitter,
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

    /// The guardrails script of the enacted constitution, folding in a constitution enacted by the
    /// volatile overlay.
    pub fn guardrail_script(&self) -> Option<Hash<SCRIPT>> {
        self.volatile.guardrail_script()
    }
}

impl<S: Store, HS: HistoricalStores + Send + 'static> State<S, HS> {
    pub fn new(
        stable: S,
        snapshots: HS,
        network: NetworkName,
        era_history: EraHistory,
        global_parameters: GlobalParameters,
        emit_initial_stake_distribution_progress_ticks: bool,
        on_startup: Option<StartupHook<S>>,
    ) -> Result<Self, StoreError> {
        let protocol_parameters = stable.protocol_parameters()?;

        protocol_version::validate(protocol_parameters.protocol_version, protocol_version::MINIMUM_SUPPORTED)
            .map_err(|e| StoreError::Internal(Box::new(e)))?;

        let governance_activity = stable.governance_activity()?;

        let guardrail_script = stable.constitution()?.guardrail_script;

        let epoch = initial_epoch(&stable, &snapshots, &era_history)?;

        if let Some(on_startup) = on_startup {
            on_startup(&StartupContext::new(&stable, epoch, &protocol_parameters, &era_history))?;
        }

        let stake_distributions = initialize_stake_distributions(
            network,
            &snapshots,
            &era_history,
            emit_initial_stake_distribution_progress_ticks,
        )?;

        Ok(Self::new_with(
            stable,
            snapshots,
            epoch,
            network,
            era_history,
            global_parameters,
            protocol_parameters,
            governance_activity,
            guardrail_script,
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
        guardrail_script: Option<Hash<SCRIPT>>,
        stake_distributions: VecDeque<StakeDistribution>,
    ) -> Self {
        // The store deletes the candidate snapshots when it opens, so the most recent snapshot on
        // disk is final.
        let global_parameters = Arc::new(global_parameters);
        let era_history = Arc::new(era_history);
        let latest_snapshot = LatestSnapshot::new(
            snapshots.most_recent_snapshot(),
            network,
            global_parameters.clone(),
            era_history.clone(),
            stake_distributions,
        );

        Self {
            stable,

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
            volatile: VolatileDB::new(epoch, protocol_parameters, governance_activity, guardrail_script),

            global_parameters,

            era_history,

            network,

            observers: LedgerObservers::default(),

            latest_snapshot,

            tip_update_emitter: TipUpdateEmitter::default(),
        }
    }

    /// Set a callback to be invoked when a new stake distribution snapshot becomes available.
    /// The callback receives the projected PoolSummaries.
    pub fn set_on_stake_dist_updated(&mut self, cb: Arc<dyn Fn(PoolSummaries) + Send + Sync>) {
        self.latest_snapshot.set_on_stake_dist_updated(cb);
    }

    /// Start the stake distribution and rewards computation that the ledger state at its tip
    /// needs, before the first block is applied.
    ///
    /// A ledger state can start with a tip 3·k/f slots or more into its epoch. The computation
    /// for that epoch must then run before the next block, because this block can cross the epoch
    /// boundary. Call this after [`Self::set_on_stake_dist_updated`] and [`Self::set_observers`],
    /// so that the computation publishes its results. A second call has no effect while the
    /// computation runs.
    pub fn start(&mut self) -> Result<(), StateError> {
        self.try_compute_stake_distribution_and_rewards(self.tip().slot_or_default())
    }

    /// Install embedder observers (adopted blocks and optional full stake summaries).
    pub fn set_observers(&mut self, observers: LedgerObservers) {
        self.observers = observers;
    }

    /// Project the small pool summaries needed for header validation (and leader schedule)
    /// from the held stake summaries. Only the `.pools` data is included.
    pub fn pool_summaries(&self) -> PoolSummaries {
        self.latest_snapshot.pool_summaries()
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

    /// The most recent epoch with a snapshot, final or candidate.
    pub fn most_recent_snapshot(&self) -> Epoch {
        self.latest_snapshot.epoch()
    }

    /// Inspect the tip of this ledger state. This corresponds to the point of the latest block
    /// applied to the ledger.
    pub fn tip(&'_ self) -> Cow<'_, Point> {
        if let Some(st) = self.volatile.view_back() {
            return Cow::Owned(st.anchor.0);
        }

        Cow::Owned(self.immutable_tip())
    }

    #[expect(clippy::panic)]
    /// Point of the immutable db (i.e. farthest point we can ever rollback to).
    pub fn immutable_tip(&self) -> Point {
        self.stable.tip().unwrap_or_else(|e| panic!("no tip found in stable db: {e:?}"))
    }

    /// Point of the volatile (`VolatileDB`) sequence only, if non-empty.
    pub fn volatile_tip(&self) -> Option<Point> {
        self.volatile.view_back().map(|fragment| fragment.tip())
    }

    /// The epoch and the last block of the candidate snapshot, if one exists and is not settled.
    #[cfg(any(test, feature = "test-utils"))]
    pub fn candidate_snapshot(&self) -> Option<(Epoch, Point)> {
        self.latest_snapshot.candidate()
    }

    /// The epoch and the last block of the snapshot that still has a directory on disk, candidate or settled.
    #[cfg(any(test, feature = "test-utils"))]
    pub fn candidate_snapshot_on_disk(&self) -> Option<(Epoch, Point)> {
        self.latest_snapshot.candidate_on_disk()
    }

    /// Points of the blocks of the closing epoch that are still volatile, oldest first.
    #[cfg(any(test, feature = "test-utils"))]
    pub fn volatile_epoch_tail(&self) -> Vec<Point> {
        self.volatile.epoch_tail_view().map(|fragment| fragment.tip()).collect()
    }

    /// Get registered relay candidates from the stable store (sockets, hostnames, and SRV names).
    ///
    /// **NOTE:** This operation blocks the ledger for about 4ms (mainnet late
    /// 2025), so it should be called with care. Please cache the result, it
    /// only changes meaningfully once per epoch.
    pub fn registered_relay_candidates(&self) -> Result<BTreeSet<amaru_kernel::PeerCandidate>, StateError> {
        Ok(crate::registered_relay_addrs::collect_from_read_store(&self.stable)?)
    }

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

                let governance_activity = self.governance_activity_for(immutable_epoch).unwrap_or_else(|| unreachable! {
                    "invariant violation: asking governance activity for an unreachable epoch; immutable epoch = {}; volatile epoch = {}",
                    immutable_epoch,
                    self.epoch(),
                });

                save_block(&self.stable, &self.era_history, protocol_parameters, governance_activity, now_stable)
                    .map_err(StateError::Storage)
            },
        )
    }

    /// Check whether the next state should cause an epoch transition. This is the case when it
    /// corresponds to a block in a different (next) epoch, in which case, we must first transition
    /// into the new epoch before the block can be validated.
    fn try_epoch_transition(&mut self, next_tip: Point) -> Result<(), StateError> {
        let next_epoch = unsafe_slot_to_epoch(&self.era_history, next_tip.slot_or_default());

        if next_epoch > self.epoch() {
            let old_protocol_version = self.protocol_version();

            self.epoch_transition(next_epoch)?;

            let new_protocol_version = self.protocol_version();

            if old_protocol_version != new_protocol_version {
                info!(
                    ledger::protocol::UPGRADE,
                    old_version = old_protocol_version.major(),
                    new_version = new_protocol_version.major()
                );
            }
        }

        Ok(())
    }

    fn epoch_transition(&mut self, next_epoch: Epoch) -> Result<(), StateError> {
        info_span!(ledger::epoch_transition::COMPUTE, from = next_epoch - 1, into = next_epoch).in_scope(|| {
            // wait for the rewards computation to be finished and get the rewards
            let rewards = self.latest_snapshot.join_task(next_epoch - 1, &self.stable)?;

            let mut computed_rewards = if let Some(rewards) = rewards {
                debug_assert!(
                    !self.volatile.has_computed_rewards(),
                    "the rewards of epoch {} were computed again, while the ledger still holds them",
                    next_epoch - 1
                );
                Some(Rewards::<Computed>::from(rewards))
            } else {
                // A fork switch that re-crosses the epoch boundary rolled the overlay's rewards
                // back from Effective to Computed; consume them for the re-transition.
                self.volatile.take_computed_rewards()
            };

            // NOTE: Forcing the flush before the next epoch boundary
            //
            // On a chain with fewer than `k` blocks in a whole epoch, the last blocks of the
            // previous epoch are still volatile now. The volatile window can only hold one epoch
            // boundary, and ratification needs a final snapshot of the previous epoch: so the
            // pending epoch transition must reach the stable store now.
            if !self.volatile.is_draining_empty() {
                self.flush_epoch_transition()?;
            }

            // NOTE: No block after the stability window
            //
            // Only a block after the stability window starts the computation. When the closing
            // epoch has no such block, the computation runs now, from the final snapshot that the
            // flush took. A computation that ran already is not started again.
            if computed_rewards.is_none() && self.stable.epoch_transition_progress()?.is_none() {
                self.compute_stake_distribution_and_rewards()?;
                computed_rewards =
                    self.latest_snapshot.join_task(next_epoch - 1, &self.stable)?.map(Rewards::<Computed>::from);
            }

            // A rollback into the epoch of a candidate snapshot discards it, so no candidate snapshot
            // of the closing epoch exists here.
            debug_assert!(
                self.latest_snapshot.is_final(),
                "no candidate snapshot survives the epoch boundary: {:?}",
                self.latest_snapshot
            );

            let db = &self.stable;

            let progress = db.epoch_transition_progress()?;

            if let Some(resuming_from) = progress {
                Span::current().record("resuming_from", resuming_from.to_string());
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
            let mut volatile_view = VolatileView::new(&self.volatile, db);

            // Compute the updates to perform on pools at the epoch boundary. This uses information
            // from both the immutable store and the volatile database, since we compute the updates
            // before they are "stable" and safe to store.
            let pools_updates = PoolsEpochTransitionUpdates::new(volatile_view.iter_pools()?, next_epoch);

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
            let (treasury, effective_rewards, unreachable_accounts) = if progress.is_none() {
                let computed_rewards = computed_rewards.ok_or(StateError::RewardsSummaryNotReady)?;

                let unreachable_accounts =
                    volatile_view.iter_unreachable_accounts(computed_rewards.pools_owners())?.collect::<BTreeSet<_>>();

                let unclaimed_rewards = computed_rewards.unclaimed_rewards(unreachable_accounts.iter().copied());

                let effective_rewards = Rewards::<Effective>::new(computed_rewards, unclaimed_rewards);

                let treasury = db.pots()?.treasury + effective_rewards.delta_treasury();

                (treasury, Some(effective_rewards), unreachable_accounts)
            } else {
                let unreachable_accounts =
                    volatile_view.iter_unreachable_accounts(BTreeSet::new())?.collect::<BTreeSet<_>>();

                (db.pots()?.treasury, None, unreachable_accounts)
            };

            let protocol_parameters = self.protocol_parameters();

            let ratification_context = RatificationContext::new(
                // Ratification happens with one epoch of delay, and at the next epoch transition. So,
                // if we ratify votes that happened in epoch `e`, the ratification is done during the
                // transition from `e + 1` to `e + 2`;
                //
                // Here, we have `next_epoch = e + 2`. And so, we have to pull the data and stake
                // distribution from at `next_epoch - 2`.
                self.snapshots.for_epoch(next_epoch - 2)?,
                self.latest_snapshot.stake_distribution(next_epoch - 2)?,
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

            self.volatile.transition(
                effective_rewards,
                pools_updates,
                governance_updates,
                volatile_view.donations()?,
                |account| !unreachable_accounts.contains(account),
            );

            Ok(())
        })
    }

    /// If `slot` is past the stability window of the epoch of the tip, start the stake
    /// distribution and rewards computation on the latest snapshot.
    ///
    /// NOTE: Epochs used by the stake distribution and rewards computation
    ///
    /// The tip is in epoch e, at least 3·k/f slots after its start. The computation reads
    /// 2 snapshots:
    ///
    /// - Snapshot e−1 (`previous_epoch`): the state at the end of epoch e−1. It is a final
    ///   snapshot, or a candidate snapshot when the last blocks of epoch e−1 are still
    ///   volatile. It gives:
    ///   - the stake distribution of epoch e−1. The computation publishes it at once,
    ///     because consensus needs it to validate the headers of epoch e+1;
    ///   - the blocks made by each pool during epoch e−1, and the pots. They set the amount
    ///     of the rewards.
    /// - Snapshot e−3 (`rewards_snapshot`): the stake that the rewards are shared on. It is
    ///   the stake distribution that elected the leaders of epoch e−1.
    ///
    /// The transition from epoch e to epoch e+1 pays these rewards.
    ///
    ///  epoch e−3 │ epoch e−2 │ epoch e−1 │     epoch e     │ epoch e+1
    /// ───────────┼───────────┼───────────┼─────────────────┼───────────
    ///            ▲                       ▲     ▲           ▲
    ///            │                       │     │           └ the transition e → e+1 pays the rewards
    ///            │                       │     └ after 3·k/f slots: the computation starts
    ///            │                       └ snapshot e−1: the blocks of each pool in epoch e−1, the pots,
    ///            │                         and the stake distribution of epoch e−1 for the headers of epoch e+1
    ///            └ snapshot e−3: the stake that the rewards are shared on
    pub fn try_compute_stake_distribution_and_rewards(&mut self, slot: Slot) -> Result<(), StateError> {
        if !self.is_epoch_stable_at(slot) {
            return Ok(());
        }
        self.compute_stake_distribution_and_rewards()
    }

    /// Start the stake distribution and rewards computation of the current epoch, from the latest
    /// snapshot.
    fn compute_stake_distribution_and_rewards(&mut self) -> Result<(), StateError> {
        // The computation must only start when the current epoch is `e` and the latest snapshot
        // is `e - 1`. But after a restart the volatile state is lost and the stable tip might be in
        // epoch `e - 1` while we already have a snapshot of `e - 1` as the latest snapshot.
        if self.latest_snapshot.epoch() >= self.current_epoch() {
            return Ok(());
        }

        self.latest_snapshot.spawn_stake_distribution_and_rewards_task(
            &self.snapshots,
            self.volatile.protocol_parameters(),
            &self.observers,
        )
    }

    /// Whether `slot` is past the stability window of the epoch of the tip: 3·k/f slots or more
    /// into that epoch, or in a later epoch.
    fn is_epoch_stable_at(&self, slot: Slot) -> bool {
        self.era_history.is_slot_stable_for(slot, &self.current_epoch())
    }

    /// Return the current epoch according to the state tip
    fn current_epoch(&self) -> Epoch {
        let tip = self.tip().slot_or_default();
        unsafe_slot_to_epoch(&self.era_history, tip)
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

    /// Once `slot` is past the stability window of the epoch of the tip, or once the closing epoch
    /// has no volatile block left: flush the epoch transition to the stable store, which takes the
    /// final snapshot of the closing epoch. If its last blocks are still volatile, take a candidate
    /// snapshot of it instead. Does nothing when that snapshot exists already.
    pub fn try_take_snapshot(&mut self, slot: Slot) -> Result<(), StateError> {
        if !self.volatile.has_pending_epoch_transition() {
            return Ok(());
        }

        if self.volatile.is_draining_empty() {
            self.flush_overlay()?;
        } else if self.latest_snapshot.is_final() && self.is_epoch_stable_at(slot) {
            self.make_candidate_snapshot()?;
        }

        Ok(())
    }

    /// Take a candidate snapshot of the closing epoch, whose last blocks are still volatile.
    ///
    /// In a healthy chain that honors the Chain Growth property, this never happens: the ledger
    /// sees `k` blocks before 3*k/f slots of the new epoch, so the last blocks of the closing epoch
    /// are stable by then. If this property is not satisfied the ledger still needs a snapshot to
    /// compute the stake distribution to validate the upcoming headers in the next era.
    ///
    /// The candidate snapshot is a copy of the stable store, with the last blocks of the closing
    /// epoch and the end of that epoch (rewards payment) written to it. The stable store and the
    /// volatile state are unchanged, so a rollback through these blocks stays possible.
    fn make_candidate_snapshot(&mut self) -> Result<(), StateError> {
        let epoch = self.volatile.epoch() - 1;

        warn!(
            ledger::chain_growth::VIOLATE,
            unstable_tail_length = self.volatile.epoch_tail_view().count(),
            reason = format!(
                "chain growth violation: less than k={k} blocks seen in a window of 3*k/f={stability_window} \
                 slots; the ledger takes a candidate snapshot of epoch {epoch}, whose last blocks are still volatile.",
                k = self.global_parameters().consensus_security_param,
                stability_window = self.global_parameters().stability_window()
            )
        );

        if let Some((epoch, last_block)) = self.write_candidate_snapshot()? {
            self.latest_snapshot.set_as_candidate(epoch, last_block);
        }

        Ok(())
    }

    /// Write the candidate snapshot of the closing epoch to disk, from the stable store and the
    /// blocks of that epoch that are still volatile. `None` when no block of that epoch is volatile.
    fn write_candidate_snapshot(&self) -> Result<Option<(Epoch, Point)>, StateError> {
        let epoch = self.volatile.epoch() - 1;

        let Some(last_block) = self.volatile.epoch_tail_view().last().map(|fragment| fragment.point()) else {
            return Ok(None);
        };

        let protocol_parameters = self.protocol_parameters_for(epoch).unwrap_or_else(|| {
            unreachable!("invariant violation: no protocol parameters for the closing epoch {epoch}")
        });
        let governance_activity = self.governance_activity_for(epoch).unwrap_or_else(|| {
            unreachable!("invariant violation: no governance activity for the closing epoch {epoch}")
        });

        let candidate = self.stable.create_candidate_snapshot(epoch, &last_block.hash())?;

        for fragment in self.volatile.epoch_tail_view() {
            save_block(&candidate, &self.era_history, protocol_parameters, governance_activity, fragment.clone())?;
        }

        candidate.with_transaction(|batch| self.volatile.end_epoch(batch))?;

        Ok(Some((epoch, last_block)))
    }

    /// Write the pending epoch transition to the stable store, with the last blocks of the closing
    /// epoch that are still volatile. After this call, a rollback through these blocks fails.
    fn flush_epoch_transition(&mut self) -> Result<(), StateError> {
        if let Some((len, epoch_tail)) = self.volatile.epoch_tail() {
            warn!(
                ledger::chain_growth::VIOLATE,
                unstable_tail_length = len,
                reason = format!(
                    "chain growth violation: less than k={k} blocks seen in a whole epoch; if this occurs \
                     during historical sync, it may not be a big problem. However If this occurs at the tip, it \
                     can be more serious. We will not be able to rollback through still-unstable blocks that \
                     must now be persisted to disk.",
                    k = self.global_parameters().consensus_security_param,
                )
            );

            for anchored_fragment in epoch_tail {
                self.apply_block(anchored_fragment)?;
            }
        }

        self.flush_overlay()
    }

    /// Write the pending epoch transition to the stable store, once the last blocks of the closing
    /// epoch are there. This takes the final snapshot of that epoch.
    fn flush_overlay(&mut self) -> Result<(), StateError> {
        self.volatile.apply_transition(&self.stable)?;
        // A settled candidate snapshot stays on disk while the stake distribution and rewards
        // computation reads it.
        self.latest_snapshot.finalize(self.volatile.epoch() - 1, &self.stable);
        self.snapshots.prune(self.volatile.epoch() - MIN_LEDGER_SNAPSHOTS)?;
        Ok(())
    }

    /// Create a validation context for a whole block.
    fn create_block_validation_context(&self, block: &Block) -> Result<DefaultValidationContext, StateError> {
        debug_span!(
            ledger::block_validation_context::CREATE,
            block_id = block.header.hash(),
            block_number = block.header.block_height().into_u64(),
            block_body_size = block.header.body().block_body_size
        )
        .in_scope(|| {
            let mut ctx = DefaultPreparationContext::new();
            rules::prepare_block(&mut ctx, block);
            let db = &self.stable;
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
    fn create_transaction_validation_context(
        &self,
        transaction: &Transaction,
        previous_fragments: &[VolatileFragment],
    ) -> Result<DefaultValidationContext, StateError> {
        let transaction_id = transaction.tx_id();
        debug_span!(ledger::transaction_validation_context::CREATE, id = transaction_id).in_scope(|| {
            let mut ctx = DefaultPreparationContext::new();
            rules::prepare_transaction(&mut ctx, &transaction.body);
            let db = &self.stable;
            let volatile = VolatileMempool::new(&self.volatile, previous_fragments.iter());
            ctx.into_validation_context(
                UnresolvedInputPolicy::Reject,
                self.volatile.proposals_roots().cloned().unwrap_or(db.proposals_roots().map_err(StateError::Storage)?),
                &volatile,
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
        previous_fragments: &[VolatileFragment],
        mode: ValidationMode,
    ) -> Result<VolatileFragment, TransactionValidationError> {
        let transaction_id = transaction.tx_id();
        let mut context = self
            .create_transaction_validation_context(transaction, previous_fragments)
            .map_err(|error| TransactionValidationError::Preparation { transaction_id, error })?;

        rules::block::validate_transaction(
            &mut context,
            arena_pool,
            self.network(),
            self.protocol_parameters(),
            self.era_history(),
            self.global_parameters(),
            self.governance_activity(),
            self.guardrail_script(),
            TransactionPointer { slot, transaction_index: 0 },
            transaction.tx_ref(),
            mode,
        )
        .map(|_| VolatileFragment::from(context))
        .map_err(|violation| TransactionValidationError::Validation { transaction_id, violation: Box::new(violation) })
    }

    /// Roll the ledger forward given a new upcoming block. This roughly unwinds the following
    /// steps:
    ///
    /// 1. **Epoch Transition**
    ///
    ///    Try to transition into a new epoch should the block make the ledger cross an epoch
    ///
    /// 2. **Validation Context**
    ///
    ///    Create a validation context from the current stable ledger state + epoch transition if any
    ///
    /// 3. **Ledger rules execution**
    ///
    ///    Runs validation rules, collecting and aggregating block updates into a single update
    ///    fragment.
    ///
    /// 4. **Record new volatile fragment**
    ///
    ///    Anchor those updates and push them into the volatile store.
    ///
    /// 5. **Apply now-stable block**
    ///
    ///    Finally, we can store the new now-stable block to the stable store.
    ///
    /// 6. **Flush epoch transition**
    ///
    ///    In normal operations (i.e. once the ledger is done warming up), pushing a new state to
    ///    the volatile automatically yields a new now-stable state that is recorded to disk.
    ///
    ///    Before attempting to record the next block from a new epoch to disk, any pending epoch
    ///    transition must be fully flushed and a snapshot taken.
    ///
    /// 7. **Stake distribution / Rewards calculation, after this block**
    ///
    ///    Start the stake distribution and rewards computation if the ledger state after this block
    ///    allows it: step 6 can take the snapshot of the previous epoch, and this block can be the
    ///    first one 3·k/f slots into the epoch. The headers of the next epoch need the stake
    ///    distribution that it computes. Consensus accepts no further block before it has this
    ///    distribution, so the computation cannot wait for the next block. [`Self::start`] starts
    ///    the computation that the ledger state needs before its first block.
    pub fn roll_forward(
        &mut self,
        block: &Block,
        arena_pool: &ArenaPool,
    ) -> BlockValidation<LedgerMetrics, anyhow::Error> {
        debug_span!(ledger::state::ROLL_FORWARD).in_scope(|| {
            let point = block.point();

            // 1. Epoch transition
            BlockValidation::from(self.try_epoch_transition(point))?;

            let issuer = PoolId::from(Hasher::<224>::hash(&block.header.body().issuer_verification_key[..]));

            let metrics = self.new_metrics(&point, block, issuer);

            // 2. Validation context
            let mut context = BlockValidation::from(self.create_block_validation_context(block))?;

            // 3. Ledger rules execution
            rules::validate_block(
                &mut context,
                arena_pool,
                self.network(),
                self.protocol_parameters(),
                self.era_history(),
                self.global_parameters(),
                self.governance_activity(),
                self.guardrail_script(),
                block,
            )?;

            // 4. Record new volatile state
            let fragment = VolatileFragment::from(context).anchor(point, issuer);
            if let Some(now_stable) = BlockValidation::from(self.push_fragment(fragment))? {
                // 5. Apply now-stable block
                BlockValidation::from(self.apply_block(now_stable))?;
            }

            // 6. Flush the epoch transition, or take a candidate snapshot
            let slot = point.slot_or_default();
            BlockValidation::from(self.try_take_snapshot(slot))?;

            // 7. Stake distribution and rewards calculation, from the latest snapshot after this block
            BlockValidation::from(self.try_compute_stake_distribution_and_rewards(slot))?;

            if self.observers.wants_block_events() {
                // Borrow UTxO DiffSet from the fragment we just pushed (still at the tip).
                #[expect(clippy::expect_used)]
                let anchored =
                    self.volatile.view_back().expect("roll_forward pushed a fragment before notifying observers");
                debug_assert_eq!(anchored.point(), point);
                let epoch = unsafe_slot_to_epoch(&self.era_history, slot);
                let adopted = crate::observers::AdoptedBlock::from_block(epoch, block, &anchored.fragment);
                self.observers.notify_adopted(adopted);
            }

            let era_history = Arc::clone(&self.era_history);
            self.tip_update_emitter.notify(Instant::now(), &point, &metrics, &era_history);

            BlockValidation::Valid(metrics)
        })
    }

    fn new_metrics(&self, point: &Point, block: &Block, issuer: PoolId) -> LedgerMetrics {
        let slot = point.slot_or_default();

        let prev_hash = block.header.body().prev_hash;

        let block_height = block.header.block_height().into_u64();

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
            tx_count: block.transaction_bodies.len() as u64,
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

    /// Try to rollback the volatile state to a given point and roll the ledger forward with the new blocks.
    /// Blocks are applied with the regular [`Self::roll_forward`], so epoch
    /// transitions, rewards and snapshots are processed exactly as during normal chain extension.
    ///
    /// A block failing to apply is reported in the returned [`ForkSwitchOutcome`] value:
    ///
    /// - If the applied prefix is shorter than the rolled-back chain and nothing was written
    ///   to the stable store, the pre-switch state is fully restored
    ///   ([`ForkSwitchOutcome::Failed`]).
    /// - If the fork is strictly longer than the chain it replaces, i.e. if some blocks have been
    ///   persisted into the stable store, the valid prefix is kept and reported as
    ///   [`ForkSwitchOutcome::Partial`].
    ///
    /// Only infrastructure failures (missing blocks, storage errors) surface as `Err`.
    pub fn switch_to_fork<I>(
        &mut self,
        fork_point: &Point,
        blocks: I,
        arena_pool: &ArenaPool,
    ) -> anyhow::Result<ForkSwitchOutcome>
    where
        I: IntoIterator<Item = Block>,
        I::IntoIter: ExactSizeIterator,
    {
        let blocks = blocks.into_iter();
        let fork_length = blocks.len();

        let _span =
            info_span!(ledger::state::SWITCH_TO_FORK, fork_point, fork_length, rollback_length = 0usize).entered();

        let initial_immutable_tip = self.immutable_tip();

        // Rollback to the fork point and snapshot the initial volatile state
        // in case we need to recover it later.
        let state_recovery = self.rollback_to(fork_point)?;

        let rollback_length = state_recovery.rollback_length();
        trace_record!(ledger::state::SWITCH_TO_FORK, fork_point, fork_length, rollback_length);

        // The fork must replace the rolled-back chain at equal length or extend it by exactly one
        // block. If this condition is violated, this means that there is an issue with chain selection.
        // We return an error to let the consensus layer deal with it.
        if fork_length < rollback_length || fork_length > rollback_length + 1 {
            error_record!(ledger::state::SWITCH_TO_FORK, outcome = "invalid fork length");
            self.recover(state_recovery)?;
            return Err(StateError::InvalidForkLength { rollback_length, fork_length }.into());
        }

        // Silence observers during replay; emit undos then adopts only after full success.
        // Keep blocks for transaction material; UTxO is borrowed from live fragments at emit time.
        let real_on_block = self.observers.on_block.take();
        let keep_blocks = real_on_block.is_some();
        let mut deferred_blocks: Vec<Block> = Vec::with_capacity(if keep_blocks { blocks.size_hint().0 } else { 0 });

        let mut applied_tip = *fork_point;
        let mut metrics = LedgerMetrics::default();

        // Try to apply each block in the fork, and stop at the first failure.
        for block in blocks {
            let block_tip = block.point();

            let result = self.roll_forward(&block, arena_pool);
            let stable_modified = self.immutable_tip() != initial_immutable_tip
                || self.latest_snapshot.is_finalized_since(&state_recovery.candidate);

            match result {
                BlockValidation::Valid(new_metrics) => {
                    if keep_blocks {
                        deferred_blocks.push(block);
                    }
                    applied_tip = block_tip;
                    metrics = new_metrics;
                }
                BlockValidation::Invalid(tip, details) => {
                    self.observers.on_block = real_on_block;
                    let failure = InvalidBlock { tip, reason: details.to_string() };

                    // The length precondition keeps every eviction behind the fork's last block,
                    // so a failed replay is always recoverable — unless an epoch transition was
                    // forced to the stable store mid-replay, which only happens when the chain
                    // violates the Chain Growth property (see `apply_transition`).
                    if !stable_modified {
                        info_record!(ledger::state::SWITCH_TO_FORK, outcome = "failed");
                        // The invalid block is the cause: keep it as the error, and the failed
                        // recovery as its context.
                        if let Err(recovery_error) = self.recover(state_recovery) {
                            return Err(anyhow::anyhow!(
                                "invalid block {} on the fork: {}",
                                failure.tip,
                                failure.reason
                            )
                            .context(format!(
                                "cannot restore the ledger after a failed fork switch: {recovery_error}"
                            )));
                        }
                        return Ok(ForkSwitchOutcome::Failed { failure });
                    }

                    warn_record!(ledger::state::SWITCH_TO_FORK, outcome = "partial");
                    self.latest_snapshot.commit(state_recovery.candidate, &self.stable);
                    return Ok(ForkSwitchOutcome::Partial { applied_tip, metrics, failure });
                }
                BlockValidation::Err(error) => {
                    self.observers.on_block = real_on_block;
                    // Restore the pre-switch state while nothing has reached the stable store.
                    // If the error is a `RewardsSummaryNotReady` we might want to retry.
                    let error = if stable_modified {
                        self.latest_snapshot.commit(state_recovery.candidate, &self.stable);
                        error
                    } else {
                        // The first error is the cause: keep it, with the failed recovery as its
                        // context.
                        match self.recover(state_recovery) {
                            Ok(()) => error,
                            Err(recovery_error) => error.context(format!(
                                "cannot restore the ledger after a failed fork switch: {recovery_error}"
                            )),
                        }
                    };
                    error_record!(ledger::state::SWITCH_TO_FORK, outcome = "error", stable_modified);
                    return Err(error);
                }
            }
        }

        // Success: restore the real handler, emit undos (tip-first, borrowed), then adopts.
        self.observers.on_block = real_on_block;
        if self.observers.wants_block_events() {
            for fragment in state_recovery.discarded_tip_first() {
                let epoch = unsafe_slot_to_epoch(&self.era_history, fragment.slot());
                let undone = crate::observers::UndoneBlock::from_anchored(fragment, epoch);
                self.observers.notify_undone(undone);
            }
        }

        // Drop recovery without restoring — new tip is committed.
        self.latest_snapshot.commit(state_recovery.candidate, &self.stable);

        if self.observers.wants_block_events() {
            for block in &deferred_blocks {
                #[expect(clippy::expect_used)]
                let anchored = self
                    .volatile
                    .iter()
                    .find(|fragment| fragment.point() == block.point())
                    .expect("fork-switch adopt block must still be in the volatile window");
                let epoch = unsafe_slot_to_epoch(&self.era_history, block.point().slot_or_default());
                let adopted = crate::observers::AdoptedBlock::from_block(epoch, block, &anchored.fragment);
                self.observers.notify_adopted(adopted);
            }
        }

        info_record!(ledger::state::SWITCH_TO_FORK, outcome = "completed");
        Ok(ForkSwitchOutcome::Completed { metrics })
    }

    /// Rollback to a previous valid point and restore the state at that point
    fn recover(&mut self, rollback_guard: RollbackGuard<'_>) -> Result<(), StateError> {
        let immutable_tip = self.immutable_tip();

        assert_eq!(
            immutable_tip, rollback_guard.immutable_tip,
            "cannot recover: immutable tip moved from {} to {} during the replay",
            rollback_guard.immutable_tip, immutable_tip,
        );

        match rollback_guard.kind {
            StateRecovery::RecoverWholeVolatileDB { volatile } => {
                self.volatile = *volatile;
            }
            StateRecovery::RecoverVolatileDBPart { recovery } => {
                self.volatile.undo_rollback(*recovery);
            }
        }

        // The replay discarded, created or removed a candidate snapshot: bring back the one that
        // matches the restored chain.
        self.latest_snapshot.recover(rollback_guard.candidate, &self.stable);

        // In case the computations for stake distribution and rewards were stopped, start them again.
        // This won't do anything if they are currently running.
        self.try_compute_stake_distribution_and_rewards(self.tip().slot_or_default())?;

        Ok(())
    }

    fn rollback_to<'a>(&mut self, to: &'a Point) -> Result<RollbackGuard<'a>, BackwardError> {
        let immutable_tip = self.immutable_tip();
        let kind = self.rollback_volatile_to(to)?;
        let candidate = self.latest_snapshot.roll_back(self.volatile.epoch());
        Ok(RollbackGuard { immutable_tip, candidate, kind })
    }

    fn rollback_volatile_to<'a>(&mut self, to: &'a Point) -> Result<StateRecovery<'a>, BackwardError> {
        info_span!(ledger::state::ROLL_BACKWARD).in_scope(|| {
            let immutable_tip = self.immutable_tip();
            let volatile_tip = self.volatile_tip().unwrap_or(immutable_tip);

            // NOTE: Rolling back to the tip of the immutable
            //
            // All rollback points within the volatile part are handled by `VolatileDB`, but there is one more
            // legal rollback target, which is the `immutable_tip()`, in which case the VolatileDB is cleared.
            if *to == immutable_tip {
                // Snapshot the whole VolatileDB fragment but leave the metadata initialized
                // for the upcoming roll forwards.
                Ok(StateRecovery::RecoverWholeVolatileDB { volatile: Box::new(self.volatile.clear()) })
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
                Ok(StateRecovery::RecoverVolatileDBPart { recovery: Box::new(recovery) })
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

/// Resolve the epoch on restart to initialize the volatile db with.
pub fn initial_epoch<S, HS>(db: &S, snapshots: &HS, era_history: &EraHistory) -> Result<Epoch, StoreError>
where
    S: Store,
    HS: HistoricalStores,
{
    let epoch_from_immutable_tip = unsafe_slot_to_epoch(era_history, db.tip()?.slot_or_default());

    // NOTE: Initial epoch on restart
    //
    // It is possible to interrupt Amaru just after the epoch transition was flushed
    // to disk. The consequence of that is: the tip of the immutable db is still in the
    // previous epoch which will cause the next block we see to trigger an epoch transition.
    //
    // However, the epoch transition had already happened and was even persisted to disk
    // already! So we must not redo it, we are already in the next epoch!
    if db.epoch_transition_progress()?.is_none() && snapshots.most_recent_snapshot() == epoch_from_immutable_tip {
        Ok(epoch_from_immutable_tip + 1)
    } else {
        Ok(epoch_from_immutable_tip)
    }
}

// RewardsCalculator
// ----------------------------------------------------------------------------

impl<S: Store, HS: HistoricalStores> State<S, HS> {
    /// Cancel and join background work. Call only after ledger processing has stopped.
    pub fn cancel_tasks(&mut self) {
        self.latest_snapshot.cancel_tasks();
    }
}

impl<S: Store, HS: HistoricalStores> Drop for State<S, HS> {
    fn drop(&mut self) {
        self.cancel_tasks();
    }
}

/// Write the changes of one block to `store`, in one transaction.
///
/// `store` is one of:
///
/// - The stable store, for a block that left the volatile window.
/// - A candidate snapshot, for a block that is still volatile. On a sparse chain, the last blocks
///   of an epoch are not stable yet when the ledger needs the snapshot of that epoch.
///
/// The caller gives the protocol parameters and the governance activity of the epoch of the block.
fn save_block(
    store: &impl Store,
    era_history: &EraHistory,
    protocol_parameters: &ProtocolParameters,
    governance_activity: GovernanceActivity,
    block: AnchoredVolatileFragment,
) -> Result<(), StoreError> {
    let StoreUpdate { point, issuer, fees, donations, add, remove, withdrawals } = block.into_store_update();

    store.with_transaction(|batch| {
        batch.save(
            era_history,
            protocol_parameters,
            Some(governance_activity),
            &point,
            Some(&issuer),
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
    /// The candidate snapshot as the rollback found it; a failed fork switch restores it.
    candidate: BeforeRollback,
    kind: StateRecovery<'a>,
}

impl RollbackGuard<'_> {
    fn rollback_length(&self) -> usize {
        self.kind.rollback_length()
    }

    /// Discarded volatile fragments in tip-first order for observer undo notifications.
    fn discarded_tip_first(&self) -> Box<dyn Iterator<Item = &AnchoredVolatileFragment> + '_> {
        self.kind.discarded_tip_first()
    }
}

#[derive(Debug)]
enum StateRecovery<'a> {
    /// A rollback to the immutable tip cleared the whole window; the entire pre-rollback volatile
    /// is moved out (via [`VolatileDB::take`]) and restored wholesale.
    RecoverWholeVolatileDB { volatile: Box<VolatileDB> },
    /// A rollback within the volatile window; only the discarded parts are captured.
    RecoverVolatileDBPart { recovery: Box<volatile::RollbackGuard<'a>> },
}

impl StateRecovery<'_> {
    fn rollback_length(&self) -> usize {
        match self {
            Self::RecoverWholeVolatileDB { volatile } => volatile.len(),
            Self::RecoverVolatileDBPart { recovery } => recovery.rollback_length(),
        }
    }

    fn discarded_tip_first(&self) -> Box<dyn Iterator<Item = &AnchoredVolatileFragment> + '_> {
        match self {
            Self::RecoverWholeVolatileDB { volatile } => Box::new(volatile.iter().rev()),
            Self::RecoverVolatileDBPart { recovery } => recovery.discarded_tip_first(),
        }
    }
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

    #[error("no suitable stake distribution for requested epoch: {0}")]
    NoSuitableStakeDistribution(Epoch),

    // TODO: Using a mere 'String' here because the source error contains some `Rc`, which aren't
    // safe to send across threads. For the sake of carrying the error around, we might want to not
    // keep Rc in errors, but clone the underlying data -- which is small anyway, in places where
    // the error is generated.
    #[error("error when ratifying proposals: {0}")]
    RatificationFailed(String),

    #[error("background task failed: task={task}")]
    BackgroundTaskFailed { task: String },

    #[error("background task cancelled during ledger shutdown")]
    BackgroundTaskCancelled,

    #[error("rewards summary not ready")]
    RewardsSummaryNotReady,

    #[error(
        "cannot switch to a fork of {fork_length} block(s) replacing {rollback_length} block(s): the fork must \
         match the replaced chain's length or exceed it by exactly one block"
    )]
    InvalidForkLength { rollback_length: usize, fork_length: usize },

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
