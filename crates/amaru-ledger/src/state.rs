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
    mem,
    net::SocketAddr,
    ops::Deref,
    sync::{Arc, Mutex, MutexGuard},
    thread::JoinHandle,
    time::{Duration, Instant},
};

use amaru_kernel::{
    Block, BlockHeight, Epoch, EraHistory, EraHistoryError, GlobalParameters, HasTransactionId, Hash, Hasher, IsHeader,
    NetworkName, Point, PoolId, ProtocolParameters, Slot, Tip, Transaction, TransactionId, TransactionPointer,
    protocol_version, size::SCRIPT, to_cbor, utils::string::display_collection,
};
use amaru_metrics::ledger::LedgerMetrics;
use amaru_observability::{debug_span, error_record, info, info_record, info_span, trace, warn, warn_record};
pub use amaru_ouroboros_traits::{ForkSwitchOutcome, InvalidBlock, PoolSummaries, PoolSummary};
use amaru_plutus::arena_pool::ArenaPool;
use num::CheckedSub;
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
    startup::{Database as StartupDatabase, StartupHook},
    state::volatile::{
        AnchoredVolatileFragment, StoreUpdate, VolatileDB, VolatileFragment, VolatileSequence, VolatileView,
    },
    store::{HistoricalStores, ReadStore, Snapshot, Store, StoreError, TransactionalContext},
    summary::{
        governance::{self, GovernanceSummary},
        rewards::RewardsSummary,
        stake_distribution::{StakeDistribution, StakeSummary},
    },
    tracing_enabled,
};

mod tip_update_emitter;
pub mod volatile;

use self::tip_update_emitter::TipUpdateEmitter;

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
    snapshots: Arc<HS>,

    /// Our own in-memory vector of volatile deltas to apply onto the stable store in due time.
    volatile: VolatileDB,

    /// Global (i.e. non-updatable) parameters of the network. This includes things like
    /// slot length, epoch length, security parameter and other pieces that cannot generally
    /// be updated but grouped here to avoid dealing with magic values everywhere.
    global_parameters: Arc<GlobalParameters>,

    /// A shared collection of the latest slim stake distributions.
    ///
    /// These are used by the runtime for leader schedule verification and governance ratification.
    /// Full stake distributions remain reconstructible from on-disk snapshots when rewards need
    /// them, which avoids retaining large account maps in steady-state memory.
    stake_distributions: Arc<Mutex<VecDeque<StakeDistribution>>>,

    /// The era history for the network this store is related to.
    era_history: Arc<EraHistory>,

    /// Which network are we connected to. This is mostly helpful for distinguishing between
    /// behavious that are network specifics (e.g. address discriminant).
    network: NetworkName,

    /// Optional callback invoked whenever a new stake distribution snapshot is added.
    /// Used to update resources and notify stages (e.g. track_peers) about fresh PoolSummaries.
    on_stake_dist_updated: Option<Arc<dyn Fn(PoolSummaries) + Send + Sync>>,

    /// Optional embedder observers (adopted blocks, full ledger stake summaries).
    observers: LedgerObservers,

    /// Background computation calculating rewards and stake distributions
    rewards_join_handle: Option<JoinHandle<Result<RewardsSummary, StateError>>>,

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

impl<S: Store, HS: HistoricalStores + Send + Sync + 'static> State<S, HS> {
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

        let stake_distributions = initial_stake_distributions(
            network,
            &snapshots,
            &era_history,
            emit_initial_stake_distribution_progress_ticks,
        )?;

        let epoch = initial_epoch(&stable, &snapshots, &era_history)?;

        if let Some(on_startup) = on_startup {
            on_startup(&StartupDatabase::new(&stable, epoch, &protocol_parameters, &era_history))?;
        }

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
        Self {
            stable: Arc::new(Mutex::new(stable)),

            snapshots: Arc::new(snapshots),

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

            global_parameters: Arc::new(global_parameters),

            stake_distributions: Arc::new(Mutex::new(stake_distributions)),

            era_history: Arc::new(era_history),

            network,

            on_stake_dist_updated: None,

            observers: LedgerObservers::default(),

            rewards_join_handle: None,

            tip_update_emitter: TipUpdateEmitter::default(),
        }
    }

    /// Set a callback to be invoked when a new stake distribution snapshot becomes available.
    /// The callback receives the projected PoolSummaries.
    pub fn set_on_stake_dist_updated(&mut self, cb: Arc<dyn Fn(PoolSummaries) + Send + Sync>) {
        self.on_stake_dist_updated = Some(cb);
    }

    /// Install embedder observers (adopted blocks and optional full stake summaries).
    pub fn set_observers(&mut self, observers: LedgerObservers) {
        self.observers = observers;
    }

    /// Project the small pool summaries needed for header validation (and leader schedule)
    /// from the held stake summaries. Only the `.pools` data is included.
    pub fn pool_summaries(&self) -> PoolSummaries {
        #[expect(clippy::unwrap_used)]
        let guard = self.stake_distributions.lock().unwrap();
        pool_summaries_for(guard.iter())
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
        self.volatile.most_recent_snapshot(self.snapshots.as_ref())
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
            let computed_rewards = if let Some(handle) = mem::take(&mut self.rewards_join_handle) {
                let task =
                    handle.join().map_err(|_| StateError::BackgroundTaskFailed { task: "rewards".to_string() })?;
                Some(Rewards::<Computed>::from(task?))
            } else {
                // A fork switch that re-crosses the epoch boundary rolled the overlay's rewards
                // back from Effective to Computed; consume them for the re-transition.
                self.volatile.take_computed_rewards()
            };

            #[allow(clippy::unwrap_used)]
            let db = self.stable.lock().unwrap();

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
            let mut volatile_view = VolatileView::new(&self.volatile, &*db);

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
            && self.rewards_join_handle.is_none()
            && Some(self.most_recent_snapshot()) == current_epoch.checked_sub(Epoch::ONE)
            && is_previous_epoch_stable
        {
            let tasks = BackgroundTasks {
                snapshots: self.snapshots.clone(),
                epoch: current_epoch,
                network: self.network,
                global_parameters: self.global_parameters().clone(),
                protocol_parameters: self.protocol_parameters().clone(),
                era_history: self.era_history().clone(),
                stake_distributions: self.stake_distributions.clone(),
                on_stake_dist_updated: self.on_stake_dist_updated.clone(),
                on_ledger_snapshot: self.observers.on_ledger_snapshot.clone(),
            };

            self.rewards_join_handle = Some(std::thread::spawn(move || {
                tasks.rotate_stake_distribution().and_then(|()| tasks.compute_rewards())
            }))
        }

        Ok(())
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
            block_id = block.header.hash(),
            block_number = block.header.block_height().into_u64(),
            block_body_size = block.header.body().block_body_size
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
        debug_span!(ledger::transaction_validation_context::CREATE, id = transaction_id).in_scope(|| {
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
            self.guardrail_script(),
            TransactionPointer { slot, transaction_index: 0 },
            transaction.tx_ref(),
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
        block: &Block,
        arena_pool: &ArenaPool,
    ) -> BlockValidation<LedgerMetrics, anyhow::Error> {
        debug_span!(ledger::state::ROLL_FORWARD).in_scope(|| {
            let tip = block.tip();
            let point = block.point();
            trace_block_transactions(&point, block.header.block_height().into_u64(), block);

            // 1. Rewards calculation
            BlockValidation::from(self.try_compute_rewards())?;

            // 2. Epoch transition
            BlockValidation::from(self.try_epoch_transition(point))?;

            let issuer = Hasher::<224>::hash(&block.header.body().issuer_verification_key[..]);

            let metrics = self.new_metrics(&point, block, issuer);

            // 3. Validation context
            let mut context = BlockValidation::from(self.create_block_validation_context(block))?;

            // 4. Ledger rules execution
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

            // 5. Record new volatile state
            let fragment = VolatileFragment::from(context).anchor(tip, issuer);
            if let Some(now_stable) = BlockValidation::from(self.push_fragment(fragment))? {
                // 6. Apply now-stable block
                BlockValidation::from(self.apply_block(now_stable))?;
            }

            // 7. Flush the epoch transition
            BlockValidation::from(self.apply_transition())?;

            if self.observers.wants_block_events() {
                // Borrow UTxO DiffSet from the fragment we just pushed (still at the tip).
                #[expect(clippy::expect_used)]
                let anchored =
                    self.volatile.view_back().expect("roll_forward pushed a fragment before notifying observers");
                debug_assert_eq!(anchored.point(), point);
                let epoch = unsafe_slot_to_epoch(&self.era_history, point.slot_or_default());
                let adopted = crate::observers::AdoptedBlock::from_block(epoch, block, &anchored.fragment);
                self.observers.notify_adopted(adopted);
            }

            let era_history = Arc::clone(&self.era_history);
            self.tip_update_emitter.notify(Instant::now(), &point, &metrics, &era_history);

            BlockValidation::Valid(metrics)
        })
    }

    fn new_metrics(&self, point: &Point, block: &Block, issuer: Hash<28>) -> LedgerMetrics {
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

        let _span = info_span!(
            ledger::state::SWITCH_TO_FORK,
            fork_point = fork_point,
            fork_length = fork_length,
            rollback_length = 0usize
        )
        .entered();

        let initial_immutable_tip = self.immutable_tip();

        // Rollback to the fork point and snapshot the initial volatile state
        // in case we need to recover it later.
        let state_recovery = self.rollback_to(fork_point)?;

        let rollback_length = state_recovery.rollback_length();
        info!(
            ledger::state::SWITCH_TO_FORK,
            fork_point = fork_point,
            fork_length = fork_length,
            rollback_length = rollback_length
        );

        // The fork must replace the rolled-back chain at equal length or extend it by exactly one
        // block. If this condition is violated, this means that there is an issue with chain selection.
        // We return an error to let the consensus layer deal with it.
        if fork_length < rollback_length || fork_length > rollback_length + 1 {
            error_record!(ledger::state::SWITCH_TO_FORK, outcome = "invalid fork length");
            self.recover(state_recovery);
            return Err(StateError::InvalidForkLength { rollback_length, fork_length }.into());
        }

        // Silence observers during replay; emit undos then adopts only after full success.
        // Keep blocks for transaction material; UTxO is borrowed from live fragments at emit time.
        let real_on_block = self.observers.on_block.take();
        let keep_blocks = real_on_block.is_some();
        let mut deferred_blocks: Vec<Block> = Vec::with_capacity(if keep_blocks { blocks.size_hint().0 } else { 0 });

        let mut applied_tip = Tip::new(initial_immutable_tip, BlockHeight::new(0));
        let mut metrics = LedgerMetrics::default();

        // Try to apply each block in the fork, and stop at the first failure.
        for block in blocks {
            let block_tip = block.tip();
            match self.roll_forward(&block, arena_pool) {
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
                    if self.immutable_tip() == initial_immutable_tip {
                        info_record!(ledger::state::SWITCH_TO_FORK, outcome = "failed");
                        self.recover(state_recovery);
                        return Ok(ForkSwitchOutcome::Failed { failure });
                    }

                    warn_record!(ledger::state::SWITCH_TO_FORK, outcome = "partial");
                    return Ok(ForkSwitchOutcome::Partial { applied_tip, metrics, failure });
                }
                BlockValidation::Err(error) => {
                    self.observers.on_block = real_on_block;
                    // Restore the pre-switch state while nothing has reached the stable store.
                    // If the error is a `RewardsSummaryNotReady` we might want to retry.
                    let stable_modified = self.immutable_tip() != initial_immutable_tip;
                    if !stable_modified {
                        self.recover(state_recovery);
                    }
                    error_record!(ledger::state::SWITCH_TO_FORK, outcome = "error", stable_modified = stable_modified);
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
        drop(state_recovery);
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
    fn recover(&mut self, rollback_guard: RollbackGuard<'_>) {
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

// NOTE: Initialize stake distribution held in-memory. The one before last is needed by the
// consensus layer to validate the leader schedule, while the one before that will be
// consumed for the rewards calculation.
//
// We always hold on two stake summaries:
//
// - The one from an epoch `e - 1` which is used for the ongoing leader schedule at epoch `e + 1`
// - The one from an epoch `e - 2` which is used for the rewards calculations at epoch `e + 1`
//
// Note that the most recent snapshot we have is necessarily `e`, since `e + 1` designates
// the ongoing epoch, not yet finished (and so, not available as snapshot).
pub fn initial_stake_distributions<HS>(
    network: NetworkName,
    snapshots: &HS,
    era_history: &EraHistory,
    emit_progress_ticks: bool,
) -> Result<VecDeque<StakeDistribution>, StoreError>
where
    HS: HistoricalStores + Send,
{
    use rayon::prelude::*;

    let epochs = {
        let latest_epoch = snapshots.most_recent_snapshot();
        let epoch_for_leader_schedule = latest_epoch.checked_sub(Epoch::ONE);
        [Some(latest_epoch), epoch_for_leader_schedule].into_iter().flatten().collect::<Vec<_>>()
    };

    for epoch in &epochs {
        info!(ledger::stake_distribution::INITIAL_BEGIN, epoch = *epoch);
    }

    let stake_distributions = epochs
        .into_iter()
        .map(|epoch| snapshots.for_epoch(epoch))
        .collect::<Result<Vec<_>, _>>()?
        .into_par_iter()
        .map(|snapshot| {
            let epoch = snapshot.epoch();
            let mut printed = Instant::now();
            compute_stake_distribution(&snapshot, network, era_history, None, |progress| {
                let now = Instant::now();
                if emit_progress_ticks && now.saturating_duration_since(printed) > Duration::from_millis(100) {
                    printed = now;
                    info!(ledger::stake_distribution::INITIAL_PROGRESS, epoch = epoch, progress);
                }
            })
        })
        .collect::<Result<VecDeque<_>, _>>()
        .map_err(|err| StoreError::Internal(err.into()))?;

    info!(
        ledger::stake_distribution::INITIAL_READY,
        epochs = display_collection(stake_distributions.iter().map(|distribution| distribution.epoch)),
    );

    Ok(stake_distributions)
}

fn compute_stake_distribution(
    snapshot: &impl Snapshot,
    network: NetworkName,
    era_history: &EraHistory,
    notify_observer: Option<&(dyn Fn(&crate::observers::LedgerStateSnapshot) + Send + Sync)>,
    notify_progress: impl FnMut(f64),
) -> Result<StakeDistribution, StateError> {
    info_span!(ledger::stake_distribution::COMPUTE, epoch = snapshot.epoch(),).in_scope(|| {
        let summary =
            StakeSummary::new(snapshot, GovernanceSummary::new(snapshot, era_history)?, network, notify_progress)
                .map_err(StateError::Storage)?;

        // Opt-in: show the full summary (incl. accounts) by reference before we only
        // retain the slim in-memory distribution. Observer clones individual fields if needed.
        if let Some(notify) = &notify_observer {
            notify(&summary);
        }

        Ok(summary.stake_distribution)
    })
}

fn pool_summaries_for<'iter>(stake_distributions: impl Iterator<Item = &'iter StakeDistribution>) -> PoolSummaries {
    let mut by_epoch = BTreeMap::new();
    for distr in stake_distributions {
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

// RewardsCalculator
// ----------------------------------------------------------------------------

struct BackgroundTasks<HS: HistoricalStores> {
    snapshots: Arc<HS>,
    epoch: Epoch,
    network: NetworkName,
    global_parameters: GlobalParameters,
    protocol_parameters: ProtocolParameters,
    era_history: EraHistory,
    stake_distributions: Arc<Mutex<VecDeque<StakeDistribution>>>,
    on_stake_dist_updated: Option<Arc<dyn Fn(PoolSummaries) + Send + Sync>>,
    on_ledger_snapshot: Option<Arc<dyn Fn(&crate::observers::LedgerStateSnapshot) + Send + Sync>>,
}

impl<HS: HistoricalStores> BackgroundTasks<HS> {
    /// Compute the stake distribution from the previous epoch now that it is stable. Note that
    /// 'epoch' refers to the current epoch, which at this point should be `k` blocks deep.
    #[expect(clippy::unwrap_used)]
    fn rotate_stake_distribution(&self) -> Result<(), StateError> {
        let snapshot = self.snapshots.for_epoch(self.epoch - 1)?;

        // Only compute it if we don't already have it; this can happen on restart.
        let should_push_summary = self
            .stake_distributions
            .lock()
            .ok()
            .and_then(|ring| ring.front().map(|distr| distr.epoch))
            .map(|epoch| epoch < snapshot.epoch())
            .unwrap_or(true);

        if should_push_summary {
            let distr = compute_stake_distribution(
                &snapshot,
                self.network,
                &self.era_history,
                self.on_ledger_snapshot.as_deref(),
                |_| {},
            )?;

            let mut stake_distributions = self.stake_distributions.lock().unwrap();

            stake_distributions.push_front(distr);
            while stake_distributions.len() > 2 {
                stake_distributions.pop_back();
            }

            info!(
                ledger::stake_distribution::ROTATE,
                available_stake_distributions = display_collection(stake_distributions.iter().map(|distr| distr.epoch)),
            );

            if let Some(notify) = &self.on_stake_dist_updated {
                let pool_summaries = pool_summaries_for(stake_distributions.iter());
                drop(stake_distributions);
                notify(pool_summaries);
            }
        }

        Ok(())
    }

    /// Compute rewards for a given epoch using an anterior stake distribution.
    fn compute_rewards(&self) -> Result<RewardsSummary, StateError> {
        let stake_distribution_from = self.epoch - 3;

        info_span!(
            ledger::rewards::COMPUTE,
            for_epoch = self.epoch,
            using_stake_distribution_from_epoch = stake_distribution_from
        )
        .in_scope(|| {
            let snapshot = self.snapshots.for_epoch(stake_distribution_from).map_err(StateError::Storage)?;

            let stake_summary = StakeSummary::new(
                &snapshot,
                GovernanceSummary::new(&snapshot, &self.era_history)?,
                self.network,
                |_| {},
            )
            .map_err(StateError::Storage)?;

            let previous_epoch = self.snapshots.for_epoch(self.epoch - 1)?;

            Ok(RewardsSummary::new(
                stake_summary,
                &self.global_parameters,
                &self.protocol_parameters,
                previous_epoch.iter_block_issuers().map_err(StateError::Storage)?.map(|(_, block)| block.slot_leader),
                previous_epoch.pots()?,
            ))
        })
    }
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

    for (index, body) in block.transaction_bodies.iter().enumerate() {
        trace!(ledger::transaction::FOUND, %point, block_height, index, id = %body.tx_id());
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

    #[error("background task failed: task={task}")]
    BackgroundTaskFailed { task: String },

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
