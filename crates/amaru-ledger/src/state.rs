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
    net::SocketAddr,
    ops::Deref,
    sync::{Arc, Mutex, MutexGuard},
};

use amaru_kernel::{
    Block, Epoch, EraHistory, EraHistoryError, GlobalParameters, HasTransactionId, Hash, Hasher, NetworkName, Point,
    PoolId, ProtocolParameters, Slot, Tip, Transaction, TransactionId, TransactionPointer, protocol_version, to_cbor,
};
use amaru_metrics::ledger::LedgerMetrics;
use amaru_observability::{info_span, trace_span};
use amaru_ouroboros_traits::{HasStakeDistribution, PoolSummary, has_stake_distribution::GetPoolError};
use amaru_plutus::arena_pool::ArenaPool;
use num::CheckedSub;
use thiserror::Error;
use tracing::{Span, info, trace, warn};

use crate::{
    context::{ContextHydratationError, DefaultPreparationContext, DefaultValidationContext, UnresolvedInputPolicy},
    epoch_transition::{self, GovernanceActivity},
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
};

pub mod diff_bind;
pub mod diff_epoch_reg;
pub mod diff_set;
pub mod volatile;

/// The minimum number of past (from the current epoch) snapshots required for the ledger to
/// operate.
pub const MIN_LEDGER_SNAPSHOTS: u64 = 3;

const EVENT_TARGET: &str = "amaru::ledger::state";

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

impl<S: Store, HS: HistoricalStores> State<S, HS> {
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
        }
    }

    /// Obtain a view of the stake distribution, to allow decoupling the ledger from other
    /// components that require access to it.
    pub fn view_stake_distribution(&self) -> impl HasStakeDistribution + use<S, HS> {
        StakeDistributionObserver { view: self.stake_distributions.clone(), era_history: self.era_history.clone() }
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

        trace_span!(amaru_observability::amaru::ledger::state::APPLY_BLOCK, point_slot = u64::from(immutable_slot)).in_scope(
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
        let current_tip = self.tip();

        let current_epoch = unsafe_slot_to_epoch(&self.era_history, current_tip.slot_or_default());
        let next_epoch = unsafe_slot_to_epoch(&self.era_history, next_tip.slot_or_default());

        if next_epoch > current_epoch {
            let old_protocol_version = self.protocol_version();

            self.epoch_transition(next_epoch)?;

            let new_protocol_version = self.protocol_version();

            if old_protocol_version != new_protocol_version {
                info!(from = old_protocol_version.0, to = new_protocol_version.0, "protocol.upgrade")
            }
        }

        // TODO: Flush ledger overlay sooner.
        //
        // This is flushing the overlay at the last moment; just before we need to apply a
        // now-stable block from the new epoch. In principle, that block has been sitting in the
        // volatile db for a while.
        //
        // Hence, we know in advanced that the overlay must be applied. In fact, there can be
        // between 1s and multiple minutes before the next block. So we could get a head start and
        // start flushing right away; instead of awaiting for the next block to arrive.
        //
        // However, we have to be careful about restarts. There are scenarios where we must
        // transition and flush immediately, before we even record the next block to disk. So
        // here is the safest moment to perform this operation. We may additionally attempt to
        // do it just after flushing a block to the db (should it be the last block of a
        // previous epoch).
        if self.volatile.is_epoch_transition_stable() {
            #[allow(clippy::unwrap_used)]
            let db = self.stable.lock().unwrap();
            self.volatile.apply_transition(&*db)?;
            self.snapshots.prune(self.volatile.epoch() - MIN_LEDGER_SNAPSHOTS)?;
        }

        Ok(())
    }

    fn epoch_transition(&mut self, next_epoch: Epoch) -> Result<(), StateError> {
        info_span!(
            amaru_observability::amaru::ledger::epoch_transition::EPOCH_TRANSITION,
            from = u64::from(next_epoch - 1),
            into = u64::from(next_epoch)
        )
        .in_scope(|| {
            // FIXME: This should eventually be a '.await', as we always expect to *eventually*
            // have some rewards summary being available. There's no way to continue progressing
            // the ledger if we don't.
            let computed_rewards = self.volatile.take_computed_rewards();

            #[allow(clippy::unwrap_used)]
            let db = self.stable.lock().unwrap();

            let progress = db.epoch_transition_progress().map_err(StateError::Storage)?;

            let protocol_parameters = self.protocol_parameters();

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
            let mut volatile_view = VolatileView::new(next_epoch - 1, protocol_parameters, &self.volatile, &*db);

            let (treasury, effective_rewards) = if progress.is_none() {
                let effective_rewards = epoch_transition::end_epoch(
                    &mut volatile_view,
                    computed_rewards.ok_or(StateError::RewardsSummaryNotReady)?,
                )?;

                (db.pots()?.treasury + effective_rewards.delta_treasury(), Some(effective_rewards))
            } else {
                (db.pots()?.treasury, None)
            };

            let ratification_context = RatificationContext::new(
                self.snapshots.for_epoch(next_epoch - 2)?,
                self.stake_distribution(next_epoch - 2)?,
                protocol_parameters.clone(),
                // NOTE: ratification treasury value
                //
                // Ratification occurs after rewards have been paid out; and thus, uses the value
                // of the treasury that already includes any unpaid rewards.
                treasury,
            )?;

            let (pools_updates, governance_updates) = epoch_transition::begin_epoch(
                &mut volatile_view,
                next_epoch,
                &self.era_history,
                protocol_parameters,
                ratification_context,
            )?;

            drop(db); // Dropping the *mutable reference*, not the *actual database* :)

            self.volatile.transition(effective_rewards, pools_updates, governance_updates);

            Ok(())
        })
    }

    fn try_compute_rewards(&mut self, next_tip: Point) -> Result<(), StateError> {
        let next_slot = next_tip.slot_or_default();
        let next_relative_slot = unsafe_slot_in_epoch(&self.era_history, next_slot);
        let next_epoch = unsafe_slot_to_epoch(&self.era_history, next_slot);

        // Once we reach the stability window, compute rewards unless we've already done so.
        let is_stake_distribution_stable = next_relative_slot >= self.global_parameters.stability_window();

        // FIXME: Asynchronous rewards calculation
        //
        // compute rewards in a thread, or in a non-blocking manner to carry on with other
        // tasks while rewards are being computed; they only need to be available at the epoch
        // boundary.
        if self.volatile.rewards_not_ready() && is_stake_distribution_stable {
            let computed = self.compute_rewards(next_epoch)?.into();
            self.volatile.set_computed_rewards(computed);
        }

        Ok(())
    }

    #[expect(clippy::unwrap_used)]
    fn compute_rewards(&mut self, current_epoch: Epoch) -> Result<RewardsSummary, StateError> {
        info_span!(amaru_observability::amaru::ledger::state::COMPUTE_REWARDS, current_epoch = u64::from(current_epoch))
            .in_scope(|| {
                let mut stake_distributions = self.stake_distributions.lock().unwrap();
                let stake_distribution =
                    stake_distributions.pop_back().ok_or(StateError::StakeDistributionNotAvailableForRewards)?;

                assert_eq!(stake_distribution.epoch, current_epoch - 3, "unexpected stake distribution for epoch");

                Span::current().record("stake_distribution_epoch", u64::from(stake_distribution.epoch));

                let snapshot = self.snapshots.for_epoch(current_epoch - 1)?;

                let rewards_summary = RewardsSummary::new(
                    &snapshot,
                    stake_distribution,
                    &self.global_parameters,
                    self.protocol_parameters(),
                )
                .map_err(StateError::Storage)?;

                if stake_distributions.front().map(|distr| distr.epoch < snapshot.epoch()).unwrap_or(true) {
                    stake_distributions.push_front(compute_stake_distribution(&snapshot, &self.era_history)?);
                }

                Ok(rewards_summary)
            })
    }

    /// Push a next state into the ledger volatile storage. Once the volatile is full (i.e. filled
    /// with `k` state updates); a push will yield a stable state to apply. Otherwise, this simply
    /// fills the volatile.
    pub fn push_fragment(
        &mut self,
        state: AnchoredVolatileFragment,
    ) -> Result<Option<AnchoredVolatileFragment>, StateError> {
        trace_span!(amaru_observability::amaru::ledger::state::PUSH_STATE).in_scope(|| {
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
                trace!(target: EVENT_TARGET, size = self.volatile.len(), "volatile.warming_up",);
                None
            };

            self.volatile.push_back(state);

            Ok(now_stable)
        })
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
        trace_span!(
            amaru_observability::amaru::ledger::state::CREATE_BLOCK_VALIDATION_CONTEXT,
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
        trace_span!(
            amaru_observability::amaru::ledger::state::CREATE_TRANSACTION_VALIDATION_CONTEXT,
            transaction_id = transaction.body.id(),
        )
        .in_scope(|| {
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
    ///    Create a validation context from the current stable ledger state + overlay if any
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
    /// 6. **Flush overlay**
    ///
    ///    In normal operations (i.e. once the ledger is done warming up), pushing a new state to
    ///    the volatile automatically yields a new now-stable state that is recorded to disk.
    ///
    ///    Before attempting to record a block from a new epoch to disk, any pending overlay must
    ///    be fully flushed and a snapshot taken.
    ///
    /// 7. **Apply now-stable block**
    ///
    ///    Finally, we can store the new now-stable block to the stable store.
    ///
    pub fn roll_forward(
        &mut self,
        point: &Point,
        block: Block,
        arena_pool: &ArenaPool,
    ) -> BlockValidation<LedgerMetrics, anyhow::Error> {
        trace_span!(amaru_observability::amaru::ledger::state::ROLL_FORWARD).in_scope(|| {
            let block_height = block.header.header_body.block_number;

            trace_block_transactions(point, block_height, &block);

            // 1. Rewards calculation
            BlockValidation::from(self.try_compute_rewards(*point))?;

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
                // 6-7. Flush overlay & Apply now-stable block
                BlockValidation::from(self.apply_block(now_stable))?;
            }

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

    pub fn rollback_to(&mut self, to: &Point) -> Result<(), BackwardError> {
        info_span!(amaru_observability::amaru::ledger::state::ROLL_BACKWARD, rollback_point = to.to_string()).in_scope(
            || {
                let immutable_tip = self.immutable_tip();
                let volatile_tip = self.volatile_tip().map(|t| t.point()).unwrap_or(immutable_tip);

                // NOTE: Rolling back to the tip of the immutable
                //
                // All rollback points within the volatile part are handled by `VolatileDB`, but there is one more
                // legal rollback target, which is the `immutable_tip()`, in which case the VolatileDB is cleared.
                if *to == immutable_tip {
                    self.volatile.clear();
                } else if *to < immutable_tip {
                    return Err(BackwardError::beyond_max(*to, volatile_tip, immutable_tip));
                } else if *to > volatile_tip {
                    return Err(BackwardError::in_the_future(*to, volatile_tip, immutable_tip));
                } else {
                    self.volatile.rollback_to(to).map_err(|rollback_point| {
                        BackwardError::unknown(*rollback_point, volatile_tip, immutable_tip)
                    })?;
                }

                Ok(())
            },
        )
    }

    // TODO: awkward `contains_volatile_point`
    //
    // This is a bit weird; but it seems that what this accessor is used for is to determine
    // whether a rollback is possible to a given point (without throwing away the entire ledger by
    // trying to rollback). So this should likely be the API `can_rollback_to` instead.
    pub fn contains_volatile_point(&self, point: &Point) -> bool {
        self.volatile.has_point(point)
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
pub fn initial_stake_distributions(
    snapshots: &impl HistoricalStores,
    era_history: &EraHistory,
) -> Result<VecDeque<StakeDistribution>, StoreError> {
    let mut stake_distributions = VecDeque::new();

    let latest_epoch = snapshots.most_recent_snapshot();
    let epoch_for_leader_schedule = latest_epoch.checked_sub(Epoch::ONE);
    let epoch_for_rewards = latest_epoch.checked_sub(Epoch::TWO);

    for (ix, epoch) in [epoch_for_rewards, epoch_for_leader_schedule, Some(latest_epoch)].into_iter().enumerate() {
        if let Some(epoch) = epoch {
            let snapshot = snapshots.for_epoch(epoch)?;
            stake_distributions.push_front(
                compute_stake_distribution(&snapshot, era_history).map_err(|err| StoreError::Internal(err.into()))?,
            );
        } else {
            warn!(
                "ignoring initial stake distribution for epoch 'e - {}', where e = {}; not available",
                2 - ix,
                latest_epoch
            );
        }
    }

    Ok(stake_distributions)
}

pub fn compute_stake_distribution(
    snapshot: &impl Snapshot,
    era_history: &EraHistory,
) -> Result<StakeDistribution, StateError> {
    info_span!(
        amaru_observability::amaru::ledger::state::COMPUTE_STAKE_DISTRIBUTION,
        epoch = u64::from(snapshot.epoch())
    )
    .in_scope(|| {
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

// HasStakeDistribution
// ----------------------------------------------------------------------------

// The 'LedgerState' trait materializes the interface required of the consensus layer in order to
// validate block headers. It allows to keep the ledger implementation rather abstract to the
// consensus in order to decouple both components.
pub struct StakeDistributionObserver {
    view: Arc<Mutex<VecDeque<StakeDistribution>>>,
    era_history: Arc<EraHistory>,
}

impl HasStakeDistribution for StakeDistributionObserver {
    #[expect(clippy::unwrap_used)]
    fn get_pool(&self, slot: Slot, pool: &PoolId) -> Result<Option<PoolSummary>, GetPoolError> {
        let epoch = self
            .era_history
            // NOTE: This function is called by the consensus when validating block headers. So in
            // theory, the slot is either within the current epoch or the next since blocks must
            // form a chain. Either the previous block is well within the current epoch, or it was
            // the last block of the previous epoch.
            //
            // Either way, we do know at this point how to forecast this slot.
            .slot_to_epoch_unchecked_horizon(slot)
            .map_err(GetPoolError::SlotToEpochConversionFailure)?
            .checked_sub(Epoch::TWO);

        let view = self.view.lock().unwrap();

        let stake_distribution = view
            .iter()
            .find(|s| Some(s.epoch) == epoch)
            .ok_or(GetPoolError::StakeDistributionNotAvailable(slot, epoch))?;

        Ok(stake_distribution.pools.get(pool).map(|st| PoolSummary {
            vrf: st.parameters.vrf,
            stake: st.stake,
            active_stake: stake_distribution.active_stake,
        }))
    }
}

fn trace_block_transactions(point: &Point, block_height: u64, block: &Block) {
    let tx_count = block.transaction_bodies.len();

    trace!(target: EVENT_TARGET, %point, block_height, tx_count, "block transactions found");

    if !tracing::enabled!(target: EVENT_TARGET, tracing::Level::TRACE) {
        return;
    }

    for (tx_index, body) in block.transaction_bodies.iter().enumerate() {
        let tx_id = body.tx_id();
        trace!(target: EVENT_TARGET, %point, block_height, tx_index, tx_id = %tx_id, "transaction found in block");
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

// See [`unsafe_slot_to_epoch`]
fn unsafe_slot_in_epoch(era_history: &EraHistory, slot: Slot) -> Slot {
    era_history
        .slot_in_epoch(slot, slot)
        .unwrap_or_else(|e| unreachable!("impossible; failed to compute relative slot from tip ({slot:?}): {e:?}"))
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
