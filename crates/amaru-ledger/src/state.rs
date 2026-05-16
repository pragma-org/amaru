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
    AsHash, Block, Epoch, EraHistory, EraHistoryError, GlobalParameters, Hash, Hasher, Lovelace,
    MemoizedTransactionOutput, NetworkName, Point, PoolId, ProtocolParameters, Slot, StakeCredential,
    StakeCredentialKind, Tip, TransactionInput,
};
use amaru_metrics::ledger::LedgerMetrics;
use amaru_observability::{info_span, trace_span};
use amaru_ouroboros_traits::{HasStakeDistribution, PoolSummary, has_stake_distribution::GetPoolError};
use amaru_plutus::arena_pool::ArenaPool;
use anyhow::{Context, anyhow};
use thiserror::Error;
use tracing::{Span, debug, error, info, trace};
use volatile_db::AnchoredVolatileState;
pub use volatile_db::VolatileState;

use crate::{
    context,
    context::DefaultValidationContext,
    epoch_transition,
    epoch_transition::{GovernanceActivity, GovernanceUpdates, PoolsEpochTransitionUpdates, RewardsState},
    governance::ratification::RatificationContext,
    rules,
    rules::block::BlockValidation,
    state::volatile_db::{StoreUpdate, VolatileDB},
    store::{HistoricalStores, ReadStore, Snapshot, Store, StoreError, TransactionalContext},
    summary::{
        governance::{self, GovernanceSummary},
        rewards::RewardsSummary,
        stake_distribution::StakeDistribution,
    },
};

pub mod diff_bind;
pub mod diff_epoch_reg;
pub mod diff_set;
pub mod volatile_db;

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

    /// The computed rewards summary to be applied on the next epoch boundary. This is computed
    /// once in the epoch, and held until the end where it is reset.
    ///
    /// It also contains the latest stake distribution computed from the previous epoch, which we
    /// hold onto the epoch boundary. In the epoch boundary, the stake distribution becomes
    /// available for the leader schedule verification, whereas the stake distribution previously
    /// used for leader schedule is moved as rewards stake.
    rewards: RewardsState,

    /// Computed pools updates that are pending application to the stable store. The value is only
    /// `Some` during the first `k` blocks of an epoch since this corresponds to the unstable part
    /// of an epoch.
    ///
    /// When present, they must be taken into account when creating the ledger validation context.
    pools_updates: Option<PoolsEpochTransitionUpdates>,

    /// The result of an epoch boundary ratification, stashed temporarily until it is stable enough
    /// to persist in the stable storage.
    governance_updates: Option<GovernanceUpdates>,

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

    /// Global (i.e. non-updatable) parameters of the network. This includes things like
    /// slot length, epoch length, security parameter and other pieces that cannot generally
    /// be updated but grouped here to avoid dealing with magic values everywhere.
    global_parameters: Arc<GlobalParameters>,

    /// Updatable protocol parameters.
    protocol_parameters: ProtocolParameters,

    /// Track the number of dormant epochs (i.e. epochs that start without any available
    /// proposals).
    governance_activity: GovernanceActivity,

    /// Which network are we connected to. This is mostly helpful for distinguishing between
    /// behavious that are network specifics (e.g. address discriminant).
    network: NetworkName,
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

        let governance_activity = stable.governance_activity()?;

        let stake_distributions = initial_stake_distributions(&snapshots, &era_history)?;

        Ok(Self::new_with(
            stable,
            snapshots,
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
            volatile: VolatileDB::default(),

            rewards: RewardsState::NotReady,

            pools_updates: None,

            governance_updates: None,

            stake_distributions: Arc::new(Mutex::new(stake_distributions)),

            era_history: Arc::new(era_history),

            global_parameters: Arc::new(global_parameters),

            protocol_parameters,

            governance_activity,

            network,
        }
    }

    /// Obtain a view of the stake distribution, to allow decoupling the ledger from other
    /// components that require access to it.
    pub fn view_stake_distribution(&self) -> impl HasStakeDistribution + use<S, HS> {
        StakeDistributionObserver { view: self.stake_distributions.clone(), era_history: self.era_history.clone() }
    }

    pub fn network(&self) -> &NetworkName {
        &self.network
    }

    pub fn era_history(&self) -> &EraHistory {
        &self.era_history
    }

    pub fn protocol_parameters(&self) -> &ProtocolParameters {
        &self.protocol_parameters
    }

    pub fn governance_activity(&self) -> &GovernanceActivity {
        &self.governance_activity
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
        self.volatile.view_back().map(|st| st.anchor.0)
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
    fn apply_block(&mut self, now_stable: AnchoredVolatileState) -> Result<(), StateError> {
        trace_span!(
            amaru_observability::amaru::ledger::state::APPLY_BLOCK,
            point_slot = u64::from(now_stable.anchor.0.slot())
        )
        .in_scope(|| {
            let stable_tip_slot = now_stable.anchor.0.slot();

            let current_epoch = self
                .era_history
                .slot_to_epoch(stable_tip_slot, stable_tip_slot)
                .map_err(|e| StateError::ErrorComputingEpoch(stable_tip_slot, e))?;

            // Persist changes for this block
            let StoreUpdate { point: stable_point, issuer: stable_issuer, fees, add, remove, withdrawals } =
                now_stable.into_store_update(current_epoch, &self.protocol_parameters);

            let db = self.stable.lock().unwrap();

            let batch = db.create_transaction();

            batch
                .save(
                    &self.era_history,
                    &self.protocol_parameters,
                    &mut self.governance_activity,
                    &stable_point,
                    Some(&stable_issuer),
                    add,
                    remove,
                    withdrawals,
                )
                .and_then(|()| {
                    batch.with_pots(|mut row| {
                        row.borrow_mut().fees += fees;
                    })?;

                    batch.commit()
                })
                .map_err(StateError::Storage)?;

            Ok(())
        })
    }

    /// Check whether the next state should cause an epoch transition. This is the case when it
    /// corresponds to a block in a different (next) epoch, in which case, we must first transition
    /// into the new epoch before the block can be validated.
    fn try_epoch_transition(&mut self, next_tip: Tip) -> Result<(), StateError> {
        if let Some(current_tip) = self.volatile_tip() {
            // NOTE: calculating current epoch from slot on block application.
            //
            // This is only safe provided the next_tip is within the foreseeable window. If this isn't
            // the case, it's a clear signal of something going very wrong in the consensus/networking
            // pipeline feeding blocks to the ledger since they'd be attempting to feed a block that is
            // many day after the last applied block!
            let unsafe_slot_to_epoch = |era_history: &EraHistory, slot: Slot| -> Epoch {
                era_history
                    .slot_to_epoch_unchecked_horizon(slot)
                    .unwrap_or_else(|e| unreachable!("impossible; failed to compute epoch from tip ({slot:?}): {e:?}"))
            };

            let current_epoch = unsafe_slot_to_epoch(&self.era_history, current_tip.slot());
            let next_epoch = unsafe_slot_to_epoch(&self.era_history, next_tip.slot());

            if next_epoch > current_epoch {
                let old_protocol_version = self.protocol_parameters.protocol_version;

                self.epoch_transition(next_epoch)?;

                if old_protocol_version != self.protocol_parameters.protocol_version {
                    info!(
                        from = old_protocol_version.0,
                        to = self.protocol_parameters.protocol_version.0,
                        "protocol.upgrade"
                    )
                }
            }
        }

        Ok(())
    }

    fn epoch_transition(&mut self, next_epoch: Epoch) -> Result<(), StateError> {
        info_span!(
            amaru_observability::amaru::ledger::state::EPOCH_TRANSITION,
            from = u64::from(next_epoch - 1),
            into = u64::from(next_epoch)
        )
        .in_scope(|| {
            let db = self.stable.lock().unwrap();

            let rewards_payouts = epoch_transition::end_epoch(
                &*db,
                // FIXME: This should eventually be an '.await', as we always expect to *eventually*
                // have some rewards summary being available. There's no way to continue progressing
                // the ledger if we don't.
                self.rewards.take().ok_or(StateError::RewardsSummaryNotReady)?,
            )?;

            let ratification_context = RatificationContext::new(
                self.snapshots.for_epoch(next_epoch - 2)?,
                self.stake_distribution(next_epoch - 2)?,
                // TODO: Pass in a mutable ref?
                self.protocol_parameters.clone(),
                // NOTE: ratification treasury value
                //
                // Ratification occurs after rewards have been paid out; and thus, uses the value
                // of the treasury that already includes any unpaid rewards.
                db.pots()?.treasury + rewards_payouts.treasury(),
            )?;

            let (pools_updates, governance_updates) =
                epoch_transition::begin_epoch(&*db, next_epoch, &self.era_history, ratification_context)?;

            self.rewards = RewardsState::Effective(rewards_payouts);
            self.pools_updates = Some(pools_updates);
            self.governance_updates = Some(governance_updates);

            Ok(())
        })
    }

    #[expect(clippy::unwrap_used)]
    fn compute_rewards(&mut self) -> Result<RewardsSummary, StateError> {
        trace_span!(amaru_observability::amaru::ledger::state::COMPUTE_REWARDS).in_scope(|| {
            let mut stake_distributions = self.stake_distributions.lock().unwrap();
            let stake_distribution =
                stake_distributions.pop_back().ok_or(StateError::StakeDistributionNotAvailableForRewards)?;

            let epoch = stake_distribution.epoch + 2;

            let snapshot = self.snapshots.for_epoch(epoch)?;
            let rewards_summary =
                RewardsSummary::new(&snapshot, stake_distribution, &self.global_parameters, &self.protocol_parameters)
                    .map_err(StateError::Storage)?;

            stake_distributions.push_front(compute_stake_distribution(
                &snapshot,
                &self.era_history,
                &self.protocol_parameters,
            )?);

            Ok(rewards_summary)
        })
    }

    /// Roll the ledger forward with the given block by applying transactions one by one, in
    /// sequence. The update stops at the first invalid transaction, if any. Otherwise, it updates
    /// the internal state of the ledger.
    pub fn forward(&mut self, next_state: AnchoredVolatileState) -> Result<(), StateError> {
        trace_span!(amaru_observability::amaru::ledger::state::FORWARD).in_scope(|| {
            self.try_epoch_transition(next_state.anchor.0)?;

            let volatile_len_before = self.volatile.len() as u64;
            let security_param = self.global_parameters.consensus_security_param;

            // Persist the next now-immutable block, which may not quite exist when we just
            // bootstrapped the system
            if self.volatile.len() >= security_param {
                let now_stable = self.volatile.pop_front().unwrap_or_else(|| {
                    unreachable!(
                        "pre-condition: self.volatile.len()={} >= consensus_security_param={}",
                        self.volatile.len(),
                        self.global_parameters.consensus_security_param
                    )
                });

                trace_span!(
                    amaru_observability::amaru::ledger::state::VOLATILE_TO_STABLE,
                    persisted_point = now_stable.anchor.0.to_string(),
                    // TODO: useless fields to VOLATILE_TO_STABLE trace?
                    //
                    // The next two fields don't look super useful?
                    //
                    // 1. The length of the volatile db always vary by exactly 1 here
                    // 2. The variation is transient, as it grows again from the 'next_state'
                    volatile_len_before = volatile_len_before,
                    volatile_len_after = volatile_len_before.saturating_sub(1),
                    k = security_param as u64
                )
                .in_scope(|| self.apply_block(now_stable))?;
            } else {
                trace!(target: EVENT_TARGET, size = self.volatile.len(), "volatile.warming_up",);
            }

            let tip = next_state.anchor.0.slot();
            let relative_slot =
                self.era_history.slot_in_epoch(tip, tip).map_err(|e| StateError::ErrorComputingEpoch(tip, e))?;

            // Once we reach the stability window, compute rewards unless we've already done so.
            //
            // FIXME: Asynchronous rewards calculation
            //
            // compute rewards in a thread, or in a non-blocking manner to carry on with other
            // tasks while rewards are being computed; they only need to be available at the epoch
            // boundary.
            let stability_window = self.global_parameters.stability_window;
            /// FIXME: Flush pools updates, rewards and governance updates to disk; and then begin
            /// rewards calculation.
            if matches!(self.rewards, RewardsState::NotReady) && relative_slot >= stability_window {
                self.rewards = RewardsState::Ready(self.compute_rewards()?);
            }

            self.volatile.push_back(next_state);

            Ok(())
        })
    }

    #[expect(clippy::unwrap_used)]
    pub fn resolve_inputs<'a>(
        &'_ self,
        ongoing_state: &VolatileState,
        inputs: impl Iterator<Item = &'a TransactionInput>,
    ) -> Result<Vec<(TransactionInput, Option<MemoizedTransactionOutput>)>, StoreError> {
        let _span = trace_span!(amaru_observability::amaru::ledger::state::RESOLVE_INPUTS);
        let _guard = _span.enter();

        let mut result = Vec::new();

        let mut resolved_from_context = 0;
        let mut resolved_from_volatile = 0;
        let mut resolved_from_db = 0;

        // TODO: perform lookup in batch, and possibly within the same transaction as other
        // required data pre-fetch.
        for input in inputs {
            let output = ongoing_state
                .resolve_input(input)
                .cloned()
                .inspect(|_| resolved_from_context += 1)
                .or_else(|| self.volatile.resolve_input(input).inspect(|_| resolved_from_volatile += 1).cloned())
                .map(|output| Ok(Some(output)))
                .unwrap_or_else(|| {
                    let db = self.stable.lock().unwrap();
                    db.utxo(input).inspect(|_| resolved_from_db += 1)
                })?;

            result.push((input.clone(), output));
        }

        tracing::Span::current().record("resolved_from_context", resolved_from_context);
        tracing::Span::current().record("resolved_from_volatile", resolved_from_volatile);
        tracing::Span::current().record("resolved_from_db", resolved_from_db);

        Ok(result)
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

    fn create_validation_context(&self, block: &Block) -> anyhow::Result<DefaultValidationContext> {
        let _span = trace_span!(
            amaru_observability::amaru::ledger::state::CREATE_VALIDATION_CONTEXT,
            block_body_hash = block.header.header_body.block_body_hash,
            block_number = block.header.header_body.block_number,
            block_body_size = block.header.header_body.block_body_size
        );
        let _guard = _span.enter();

        let mut ctx = context::DefaultPreparationContext::new();
        rules::prepare_block(&mut ctx, block);
        Span::current().record("total_inputs", ctx.utxo.len());

        // TODO: Eventually move into a separate function, or integrate within the ledger instead
        // of the current .resolve_inputs; once the latter is no longer needed for the state
        // construction.
        let inputs = self
            .resolve_inputs(&Default::default(), ctx.utxo.into_iter())
            .context("Failed to resolve inputs")?
            .into_iter()
            // NOTE:
            // It isn't okay to just fail early here because we may be missing UTxO even on valid
            // transactions! Indeed, since we only have access to the _current_ volatile DB and the
            // immutable DB. That means, we can't be aware of UTxO created and used within the block.
            //
            // Those will however be produced during the validation, and be tracked by the
            // validation context.
            //
            // Hence, we *must* defer errors here until the moment we do expect the UTxO to be
            // present.
            .filter_map(|(input, opt_output)| opt_output.map(|output| (input, output)))
            .collect();

        Ok(DefaultValidationContext::new(inputs))
    }

    /// Returns:
    /// * `Ok(u64)` - if no error occurred and the block is valid. `u64` is the block height.
    /// * `Err(<InvalidBlockDetails>)` - if the block is invalid.
    /// * `Err(_)` - if another error occurred.
    pub fn roll_forward(
        &mut self,
        point: &Point,
        block: Block,
        arena_pool: &ArenaPool,
    ) -> BlockValidation<LedgerMetrics, anyhow::Error> {
        trace_span!(amaru_observability::amaru::ledger::state::ROLL_FORWARD).in_scope(|| {
            let mut context = match self.create_validation_context(&block) {
                Ok(context) => context,
                Err(e) => return BlockValidation::Err(anyhow!(e)),
            };

            let block_height = block.header.header_body.block_number;

            let issuer = Hasher::<224>::hash(&block.header.header_body.issuer_vkey[..]);

            let metrics = self.new_metrics(point, &block, issuer);

            rules::validate_block(
                &mut context,
                arena_pool,
                self.network(),
                self.protocol_parameters(),
                self.era_history(),
                self.governance_activity(),
                block,
            )?;

            let state: VolatileState = context.into();

            let tip = Tip::new(*point, block_height.into());

            match self.forward(state.anchor(tip, issuer)) {
                Ok(()) => BlockValidation::Valid(metrics),
                Err(e) => {
                    error!(%e, "Failed to roll forward the ledger state");
                    BlockValidation::Err(anyhow!(e))
                }
            }
        })
    }

    fn new_metrics(&self, point: &Point, block: &Block, issuer: Hash<28>) -> LedgerMetrics {
        let slot = point.slot_or_default();

        let prev_hash = block.header.header_body.prev_hash;

        let block_height = block.header.header_body.block_number;

        let txs_processed = block.transaction_bodies.len() as u64;

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
            txs_processed,
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
                // NOTE: Rolling back to the tip of the immutable
                //
                // On start-up where the consensus layer will typically ask the ledger to rollback
                // to the last known point, which ought to be the tip of the (immutable) database.
                //
                // Said differently, if the volatile db is empty, the rollback point MUST be the
                // tip of the immutable.
                let tip = match self.volatile_tip() {
                    Some(volatile_tip) => volatile_tip.point(),
                    None => {
                        let immutable_tip = self.immutable_tip();

                        if &immutable_tip != to {
                            return Err(BackwardError::UnknownRollbackPoint {
                                rollback_point: *to,
                                tip: immutable_tip,
                            });
                        }

                        immutable_tip
                    }
                };

                // Would still be caught by the next check, but this is a special case for which we
                // can provide a better error.
                if to > &tip {
                    return Err(BackwardError::RollbackPointInFuture { rollback_point: *to, tip });
                }

                self.volatile.rollback_to(to).map_err(|rollback_point| BackwardError::UnknownRollbackPoint {
                    rollback_point: *rollback_point,
                    tip,
                })
            },
        )
    }

    pub fn contains_volatile_point(&self, point: &Point) -> bool {
        self.volatile.contains(point)
    }

    /// Calculate chain density over the last `k` blocks (or oldest block in the volatileDB) given some `Point`.
    /// If the `Point` is older than the oldest block in the volatileDB, density is 0
    pub fn chain_density(&self, point: &Point) -> f64 {
        let latest_slot = point.slot_or_default();
        let k_slot =
            self.volatile.view_front().map(|state| state.anchor.0.point()).unwrap_or(Point::Origin).slot_or_default();

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
    let latest_epoch = snapshots.most_recent_snapshot();

    let mut stake_distributions = VecDeque::new();

    let epoch_for_rewards = latest_epoch - Epoch::from(2);
    let epoch_for_leader_schedule = latest_epoch - Epoch::from(1);

    for epoch in [epoch_for_rewards, epoch_for_leader_schedule] {
        let snapshot = snapshots.for_epoch(Epoch::from(epoch))?;

        let protocol_parameters = snapshot.protocol_parameters()?;

        stake_distributions.push_front(
            compute_stake_distribution(&snapshot, era_history, &protocol_parameters)
                .map_err(|err| StoreError::Internal(err.into()))?,
        );
    }

    Ok(stake_distributions)
}

pub fn compute_stake_distribution(
    snapshot: &impl Snapshot,
    era_history: &EraHistory,
    protocol_parameters: &ProtocolParameters,
) -> Result<StakeDistribution, StateError> {
    info_span!(
        amaru_observability::amaru::ledger::state::COMPUTE_STAKE_DISTRIBUTION,
        epoch = u64::from(snapshot.epoch())
    )
    .in_scope(|| {
        StakeDistribution::new(snapshot, protocol_parameters, GovernanceSummary::new(snapshot, era_history)?)
            .map_err(StateError::Storage)
    })
}

// Operations on the state
// ----------------------------------------------------------------------------

pub fn reset_fees<'store>(db: &impl TransactionalContext<'store>) -> Result<(), StoreError> {
    let _span = trace_span!(amaru_observability::amaru::ledger::state::RESET_FEES);
    let _guard = _span.enter();

    db.with_pots(|mut row| {
        row.borrow_mut().fees = 0;
    })
}

pub fn reset_blocks_count<'store>(db: &impl TransactionalContext<'store>) -> Result<(), StoreError> {
    let _span = trace_span!(amaru_observability::amaru::ledger::state::RESET_BLOCKS_COUNT);
    let _guard = _span.enter();

    // TODO: If necessary, come up with a more efficient way of dropping a "table".
    // RocksDB does support batch-removing of key ranges, but somehow, not in a
    // transactional way. So it isn't as trivial to implement as it may seem.
    db.with_block_issuers(|iterator| {
        for (_, mut row) in iterator {
            *row.borrow_mut() = None;
        }
    })
}

/// Return deposits back to reward accounts.
pub fn refund_many<'store>(
    db: &impl TransactionalContext<'store>,
    mut refunds: impl Iterator<Item = (StakeCredential, Lovelace)>,
) -> Result<(), StateError> {
    let leftovers = refunds.try_fold::<_, _, Result<_, StoreError>>(0, |leftovers, (account, deposit)| {
        debug!(
            target: EVENT_TARGET,
            type = %StakeCredentialKind::from(&account),
            account = %account.as_hash(),
            %deposit,
            "refund"
        );

        Ok(leftovers + db.refund(&account, deposit)?)
    })?;

    if leftovers > 0 {
        debug!(target: EVENT_TARGET, ?leftovers, "refund");
        db.with_pots(|mut pots| pots.borrow_mut().treasury += leftovers)?;
    }

    Ok(())
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
            - 2;
        let view = self.view.lock().unwrap();
        let stake_distribution =
            view.iter().find(|s| s.epoch == epoch).ok_or(GetPoolError::StakeDistributionNotAvailable(epoch))?;

        Ok(stake_distribution.pools.get(pool).map(|st| PoolSummary {
            vrf: st.parameters.vrf,
            stake: st.stake,
            active_stake: stake_distribution.active_stake,
        }))
    }
}

// Errors
// ----------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum BackwardError {
    /// The ledger has been instructed to rollback to an unknown point. This should be impossible
    /// if chain-sync messages (roll-forward and roll-backward) are all passed to the ledger.
    #[error("error rolling back to unknown point at {rollback_point}; current ledger tip is at {tip}")]
    UnknownRollbackPoint { rollback_point: Point, tip: Point },

    #[error("cannot roll back to a point {rollback_point} in the future of the current ledger tip {tip}")]
    RollbackPointInFuture { rollback_point: Point, tip: Point },
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

    #[error("failed to compute epoch from slot {0:?}: {1}")]
    ErrorComputingEpoch(Slot, EraHistoryError),
}

impl From<governance::Error> for StateError {
    fn from(origin: governance::Error) -> Self {
        match origin {
            governance::Error::EraHistoryError(slot, err) => StateError::ErrorComputingEpoch(slot, err),
            governance::Error::StoreError(err) => StateError::Storage(err),
        }
    }
}
