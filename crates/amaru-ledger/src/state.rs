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

use crate::{
    context,
    context::DefaultValidationContext,
    governance::ratification::{self, RatificationContext},
    rules,
    rules::block::BlockValidation,
    state::{
        ratification::{ProposalsRoots, ProposalsRootsRc, RatificationResult},
        volatile_db::{StoreUpdate, VolatileDB},
    },
    store::{
        EpochTransitionProgress, GovernanceActivity, HistoricalStores, ReadStore, Snapshot, Store,
        StoreError, TransactionalContext,
        columns::{pools, proposals},
    },
    summary::{
        governance::{self, GovernanceSummary},
        into_safe_ratio,
        rewards::RewardsSummary,
        stake_distribution::StakeDistribution,
    },
};
use amaru_kernel::{
    AsHash, Block, ComparableProposalId, ConstitutionalCommitteeStatus, Epoch, EraHistory,
    EraHistoryError, GlobalParameters, Hasher, Lovelace, MemoizedTransactionOutput, NetworkName,
    Point, PoolId, ProtocolParameters, Slot, StakeCredential, StakeCredentialKind,
    TransactionInput, expect_stake_credential,
};
use amaru_metrics::ledger::LedgerMetrics;
use amaru_observability::trace as observability_trace;
use amaru_ouroboros_traits::{
    HasStakeDistribution, PoolSummary, has_stake_distribution::GetPoolError,
};
use amaru_plutus::arena_pool::ArenaPool;
use anyhow::{Context, anyhow};
use std::{
    borrow::Cow,
    cmp::max,
    collections::{BTreeMap, VecDeque},
    ops::Deref,
    sync::{Arc, Mutex, MutexGuard},
};
use thiserror::Error;
use tracing::{Span, debug, error, info, trace, warn};
use volatile_db::AnchoredVolatileState;

pub use volatile_db::VolatileState;

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
    rewards_summary: Option<RewardsSummary>,

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

            rewards_summary: None,

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
        StakeDistributionObserver {
            view: self.stake_distributions.clone(),
            era_history: self.era_history.clone(),
        }
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
    #[expect(clippy::panic)]
    #[expect(clippy::unwrap_used)]
    pub fn tip(&'_ self) -> Cow<'_, Point> {
        if let Some(st) = self.volatile.view_back() {
            return Cow::Borrowed(&st.anchor.0);
        }

        Cow::Owned(
            self.stable
                .lock()
                .unwrap()
                .tip()
                .unwrap_or_else(|e| panic!("no tip found in stable db: {e:?}")),
        )
    }

    #[expect(clippy::unwrap_used)]
    #[observability_trace(amaru::ledger::state::APPLY_BLOCK, point_slot = u64::from(now_stable.anchor.0.slot_or_default()))]
    fn apply_block(&mut self, now_stable: AnchoredVolatileState) -> Result<(), StateError> {
        let stable_tip_slot = now_stable.anchor.0.slot_or_default();

        let mut db = self.stable.lock().unwrap();

        let latest_stored_tip = db.tip().map_err(StateError::Storage)?;
        let latest_stored_tip_slot = latest_stored_tip.slot_or_default();

        let current_epoch = self
            .era_history
            .slot_to_epoch(stable_tip_slot, stable_tip_slot)
            .map_err(|e| StateError::ErrorComputingEpoch(stable_tip_slot, e))?;

        let latest_stored_epoch = self
            .era_history
            .slot_to_epoch(latest_stored_tip_slot, stable_tip_slot)
            .map_err(|e| StateError::ErrorComputingEpoch(latest_stored_tip_slot, e))?;

        let epoch_transitioning = current_epoch > latest_stored_epoch;

        // The volatile sequence may contain points belonging to two epochs.
        //
        // We cross an epoch boundary as soon as the 'now_stable' block belongs to a different
        // epoch than the previously applied block (i.e. the tip of the stable storage).
        if epoch_transitioning {
            let old_protocol_version = self.protocol_parameters.protocol_version;

            let rewards_summary = self.rewards_summary.take();

            let protocol_parameters =
                self.epoch_transition(&mut *db, &self.snapshots, current_epoch, rewards_summary)?;

            self.protocol_parameters = protocol_parameters;
            self.governance_activity = db.governance_activity()?;

            if old_protocol_version != self.protocol_parameters.protocol_version {
                info!(
                    from = old_protocol_version.0,
                    to = self.protocol_parameters.protocol_version.0,
                    "protocol.upgrade"
                )
            }
        }

        // Persist changes for this block
        let StoreUpdate {
            point: stable_point,
            issuer: stable_issuer,
            fees,
            add,
            remove,
            withdrawals,
        } = now_stable.into_store_update(current_epoch, &self.protocol_parameters);

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

                // Reset the epoch transition progress once we've successfully applied the first
                // block of the next epoch.
                if epoch_transitioning {
                    let success = batch
                        .try_epoch_transition(Some(EpochTransitionProgress::EpochStarted), None)?;
                    if !success {
                        unreachable!("epoch transition reset did not succeed after first block!")
                    }
                }

                batch.commit()
            })
            .map_err(StateError::Storage)?;

        Ok(())
    }

    #[observability_trace(amaru::ledger::state::EPOCH_TRANSITION, from = u64::from(next_epoch - 1), into = u64::from(next_epoch))]
    fn epoch_transition(
        &self,
        db: &mut impl Store,
        snapshots: &impl HistoricalStores,
        next_epoch: Epoch,
        rewards_summary: Option<RewardsSummary>,
    ) -> Result<ProtocolParameters, StateError> {
        // ---------------------------------------------------------------------------- End of epoch
        let batch = db.create_transaction();
        let should_end_epoch =
            batch.try_epoch_transition(None, Some(EpochTransitionProgress::EpochEnded))?;
        if should_end_epoch {
            end_epoch(
                &batch,
                // FIXME: This should eventually be an '.await', as we always expect to *eventually*
                // have some rewards summary being available. There's no way to continue progressing
                // the ledger if we don't.
                rewards_summary.ok_or(StateError::RewardsSummaryNotReady)?,
            )
            .map_err(StateError::Storage)?;
        }
        batch.commit()?;

        // -------------------------------------------------------------------------------- Snapshot
        let batch = db.create_transaction();
        let should_snapshot = batch.try_epoch_transition(
            Some(EpochTransitionProgress::EpochEnded),
            Some(EpochTransitionProgress::SnapshotTaken),
        )?;
        let treasury = if should_snapshot {
            let treasury = db.pots()?.treasury;
            db.next_snapshot(next_epoch - 1)?;
            Ok::<_, StateError>(treasury)
        } else {
            Ok(snapshots.for_epoch(next_epoch - 1)?.pots()?.treasury)
        }?;
        batch.commit()?;
        snapshots.prune(next_epoch - MIN_LEDGER_SNAPSHOTS)?;

        // -------------------------------------------------------------------------- Start of epoch
        let batch = db.create_transaction();
        let should_begin_epoch = batch.try_epoch_transition(
            Some(EpochTransitionProgress::SnapshotTaken),
            Some(EpochTransitionProgress::EpochStarted),
        )?;

        let ratification_context = new_ratification_context(
            self.snapshots.for_epoch(next_epoch - 2)?,
            self.stake_distribution(next_epoch - 2)?,
            self.protocol_parameters.clone(),
            treasury,
        )?;

        let protocol_parameters = if should_begin_epoch {
            begin_epoch(
                &batch,
                next_epoch,
                &self.era_history,
                ratification_context,
                // Get all proposals to ratify / enact. Note that, even though the ratification happens
                // with an epoch of delay (and thus, using data from a snapshot), we always use the most
                // recent set of proposals available. While recently submitted proposals won't have any
                // votes, they might still end up being pruned due to a previous proposal being enacted.
                //
                // FIXME: We shouldn't collect all proposals here, but provides iterators for the
                // ratification step to go over them lazily.
                db.iter_proposals()?.collect::<Vec<_>>(),
                db.proposals_roots()?,
                &self.protocol_parameters,
            )
        } else {
            Ok(db.protocol_parameters()?)
        }?;
        batch.commit()?;

        Ok(protocol_parameters)
    }

    #[expect(clippy::unwrap_used)]
    #[observability_trace(amaru::ledger::state::COMPUTE_REWARDS)]
    fn compute_rewards(&mut self) -> Result<RewardsSummary, StateError> {
        let mut stake_distributions = self.stake_distributions.lock().unwrap();
        let stake_distribution = stake_distributions
            .pop_back()
            .ok_or(StateError::StakeDistributionNotAvailableForRewards)?;

        let epoch = stake_distribution.epoch + 2;

        let snapshot = self.snapshots.for_epoch(epoch)?;
        let rewards_summary = RewardsSummary::new(
            &snapshot,
            stake_distribution,
            &self.global_parameters,
            &self.protocol_parameters,
        )
        .map_err(StateError::Storage)?;

        stake_distributions.push_front(compute_stake_distribution(
            &snapshot,
            &self.era_history,
            &self.protocol_parameters,
        )?);

        Ok(rewards_summary)
    }

    /// Roll the ledger forward with the given block by applying transactions one by one, in
    /// sequence. The update stops at the first invalid transaction, if any. Otherwise, it updates
    /// the internal state of the ledger.
    #[observability_trace(amaru::ledger::state::FORWARD)]
    pub fn forward(&mut self, next_state: AnchoredVolatileState) -> Result<(), StateError> {
        // Persist the next now-immutable block, which may not quite exist when we just
        // bootstrapped the system
        if self.volatile.len() >= self.global_parameters.consensus_security_param {
            let now_stable = self.volatile.pop_front().unwrap_or_else(|| {
                unreachable!("pre-condition: self.volatile.len() >= consensus_security_param")
            });

            self.apply_block(now_stable)?;
        } else {
            trace!(target: EVENT_TARGET, size = self.volatile.len(), "volatile.warming_up",);
        }

        let tip = next_state.anchor.0.slot_or_default();
        let relative_slot = self
            .era_history
            .slot_in_epoch(tip, tip)
            .map_err(|e| StateError::ErrorComputingEpoch(tip, e))?;

        // Once we reach the stability window, compute rewards unless we've already done so.
        //
        // FIXME: compute rewards in a thread, or in a non-blocking manner to carry on with other
        // tasks while rewards are being computed; they only need to be available at the epoch
        // boundary.
        let stability_window = self.global_parameters.stability_window;
        if self.rewards_summary.is_none() && relative_slot >= stability_window {
            self.rewards_summary = Some(self.compute_rewards()?);
        }

        self.volatile.push_back(next_state);

        Ok(())
    }

    #[expect(clippy::unwrap_used)]
    #[observability_trace(amaru::ledger::state::RESOLVE_INPUTS)]
    pub fn resolve_inputs<'a>(
        &'_ self,
        ongoing_state: &VolatileState,
        inputs: impl Iterator<Item = &'a TransactionInput>,
    ) -> Result<Vec<(TransactionInput, Option<MemoizedTransactionOutput>)>, StoreError> {
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
                .or_else(|| {
                    self.volatile
                        .resolve_input(input)
                        .inspect(|_| resolved_from_volatile += 1)
                        .cloned()
                })
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
        let guard = self
            .stake_distributions
            .lock()
            .map_err(|_| StateError::FailedToAcquireStakeDistrLock)?;

        StakeDistributionView::new(guard, epoch)
    }

    #[observability_trace(amaru::ledger::state::CREATE_VALIDATION_CONTEXT,
        block_body_hash = format!("{}", block.header.header_body.block_body_hash),
        block_number = block.header.header_body.block_number,
        block_body_size = block.header.header_body.block_body_size
    )]
    fn create_validation_context(&self, block: &Block) -> anyhow::Result<DefaultValidationContext> {
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
    #[observability_trace(amaru::ledger::state::ROLL_FORWARD)]
    pub fn roll_forward(
        &mut self,
        point: &Point,
        block: Block,
        arena_pool: &ArenaPool,
    ) -> BlockValidation<LedgerMetrics, anyhow::Error> {
        let mut context = match self.create_validation_context(&block) {
            Ok(context) => context,
            Err(e) => return BlockValidation::Err(anyhow!(e)),
        };

        let block_height = block.header.header_body.block_number;
        let issuer = Hasher::<224>::hash(&block.header.header_body.issuer_vkey[..]);
        let txs_processed = block.transaction_bodies.len() as u64;

        match rules::validate_block(
            &mut context,
            arena_pool,
            self.network(),
            self.protocol_parameters(),
            self.era_history(),
            self.governance_activity(),
            block,
        ) {
            BlockValidation::Err(err) => BlockValidation::Err(err),
            BlockValidation::Invalid(slot, id, err) => BlockValidation::Invalid(slot, id, err),
            BlockValidation::Valid(()) => {
                let state: VolatileState = context.into();
                let slot = point.slot_or_default();
                let epoch = match self.era_history().slot_to_epoch(slot, slot) {
                    Ok(epoch) => epoch,
                    Err(_) => 0.into(),
                }
                .into();
                let slot_in_epoch = match self.era_history().slot_in_epoch(slot, slot) {
                    Ok(slot) => slot,
                    Err(_) => 0.into(),
                }
                .into();

                let slot = slot.into();

                let density = self.chain_density(point);

                let metrics = LedgerMetrics {
                    block_height,
                    txs_processed,
                    slot,
                    slot_in_epoch,
                    epoch,
                    density,
                };

                match self.forward(state.anchor(point, issuer)) {
                    Ok(()) => BlockValidation::Valid(metrics),
                    Err(e) => {
                        error!(%e, "Failed to roll forward the ledger state");
                        BlockValidation::Err(anyhow!(e))
                    }
                }
            }
        }
    }

    #[observability_trace(amaru::ledger::state::ROLL_BACKWARD)]
    pub fn rollback_to(&mut self, to: &Point) -> Result<(), BackwardError> {
        // NOTE: This happens typically on start-up; The consensus layer will typically ask us to
        // rollback to the last known point, which ought to be the tip of the database.
        if self.volatile.is_empty() && self.tip().as_ref() == to {
            return Ok(());
        }

        self.volatile
            .rollback_to(to, |point| BackwardError::UnknownRollbackPoint(*point))
    }
    /// Calculate chain density over the last `k` blocks (or oldest block in the volatileDB) given some `Point`.
    /// If the `Point` is older than the oldest block in the volatileDB, density is 0
    pub fn chain_density(&self, point: &Point) -> f64 {
        let latest_slot = point.slot_or_default();
        let k_slot = self
            .volatile
            .view_front()
            .map(|state| &state.anchor.0)
            .unwrap_or(&Point::Origin)
            .slot_or_default();

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

#[observability_trace(amaru::ledger::state::COMPUTE_STAKE_DISTRIBUTION, epoch = u64::from(snapshot.epoch()))]
pub fn compute_stake_distribution(
    snapshot: &impl Snapshot,
    era_history: &EraHistory,
    protocol_parameters: &ProtocolParameters,
) -> Result<StakeDistribution, StateError> {
    StakeDistribution::new(
        snapshot,
        protocol_parameters,
        GovernanceSummary::new(snapshot, era_history)?,
    )
    .map_err(StateError::Storage)
}

// Epoch Transitions
// ----------------------------------------------------------------------------

#[observability_trace(amaru::ledger::state::END_EPOCH)]
fn end_epoch<'store>(
    db: &impl TransactionalContext<'store>,
    mut rewards_summary: RewardsSummary,
) -> Result<(), StoreError> {
    // Pay rewards to each account.
    db.with_accounts(|iterator| {
        for (account, mut row) in iterator {
            if let Some(rewards) = rewards_summary.extract_rewards(&account) {
                // The condition avoids the mutable borrow when not needed, which will incur a db
                // operation.
                if rewards > 0
                    && let Some(account) = row.borrow_mut()
                {
                    account.rewards += rewards;
                }
            }
        }
    })?;

    // Adjust treasury and reserves accordingly.
    db.with_pots(|mut row| {
        let pots = row.borrow_mut();
        pots.treasury += rewards_summary.delta_treasury() + rewards_summary.unclaimed_rewards();
        pots.reserves -= rewards_summary.delta_reserves();
    })?;

    Ok(())
}

#[observability_trace(amaru::ledger::state::BEGIN_EPOCH)]
fn begin_epoch<'store>(
    db: &impl TransactionalContext<'store>,
    epoch: Epoch,
    era_history: &EraHistory,
    ctx: RatificationContext<'_>,
    proposals: Vec<(ComparableProposalId, proposals::Row)>,
    roots: ProposalsRoots,
    protocol_parameters: &ProtocolParameters,
) -> Result<ProtocolParameters, StateError> {
    // Reset counters before the epoch begins.
    reset_blocks_count(db)?;
    reset_fees(db)?;

    // Tick pools to compute their new state at the epoch boundary. Notice
    // how we tick with the _current epoch_ however, but we take the snapshot before
    // the tick since the actions are only effective once the epoch is crossed.
    //
    // FIXME: We also need a mechanism to remove any remaining delegation to pools retired by this
    // step. The accounts are already filtered out when computing rewards, but if any retired pool
    // were to re-register, they would automatically be granted the stake associated to their past
    // delegates.
    tick_pools(db, epoch, protocol_parameters)?;

    // Ratify and enact proposals at the epoch boundary. Also refund deposit for any proposal that
    // has expired.
    let protocol_parameters = tick_proposals(db, epoch, era_history, ctx, proposals, roots)?;

    Ok(protocol_parameters)
}

// Operations on the state
// ----------------------------------------------------------------------------

#[observability_trace(amaru::ledger::state::RESET_FEES)]
pub fn reset_fees<'store>(db: &impl TransactionalContext<'store>) -> Result<(), StoreError> {
    db.with_pots(|mut row| {
        row.borrow_mut().fees = 0;
    })
}

#[observability_trace(amaru::ledger::state::RESET_BLOCKS_COUNT)]
pub fn reset_blocks_count<'store>(
    db: &impl TransactionalContext<'store>,
) -> Result<(), StoreError> {
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
    let leftovers =
        refunds.try_fold::<_, _, Result<_, StoreError>>(0, |leftovers, (account, deposit)| {
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

#[observability_trace(amaru::ledger::state::TICK_POOL)]
pub fn tick_pools<'store>(
    db: &impl TransactionalContext<'store>,
    epoch: Epoch,
    protocol_parameters: &ProtocolParameters,
) -> Result<(), StateError> {
    let mut refunds = Vec::new();

    db.with_pools(|iterator| {
        for (_, pool) in iterator {
            if let Some(refund) = pools::Row::tick(pool, epoch) {
                refunds.push(refund)
            }
        }
    })?;

    refund_many(
        db,
        refunds
            .into_iter()
            .map(|credential| (credential, protocol_parameters.stake_pool_deposit)),
    )
}

#[observability_trace(amaru::ledger::state::TICK_PROPOSALS, proposals_count = proposals.len() as u64)]
pub fn tick_proposals<'store>(
    db: &impl TransactionalContext<'store>,
    epoch: Epoch,
    era_history: &EraHistory,
    ctx: RatificationContext<'_>,
    proposals: Vec<(ComparableProposalId, proposals::Row)>,
    roots: ProposalsRoots,
) -> Result<ProtocolParameters, StateError> {
    let mut refunds: BTreeMap<StakeCredential, Lovelace> = BTreeMap::new();

    let RatificationResult {
        context: ctx,
        store_updates,
        pruned_proposals,
    } = ctx
        .ratify_proposals(era_history, proposals, ProposalsRootsRc::from(roots))
        .map_err(|e| StateError::RatificationFailed(e.to_string()))?;

    store_updates
        .into_iter()
        .try_for_each(|apply_changes| apply_changes(db, &ctx))?;

    let mut still_active = 0;
    db.with_proposals(|iterator| {
        for (key, mut item) in iterator {
            if let Some(row) = item.borrow() {
                // This '+2' is worthy of an explanation.
                //
                // - `epoch` here designates the _next_ epoch we are transitioning into.
                //
                // - So, `epoch - 1` points at the epoch that _just ended_.
                //
                // - Proposals "valid_until" epoch `e` means that they expire during the
                //   transition from `e` to `e + 1`  (they can still be voted on in `e`!)
                //
                // - Proposals are processed with an epoch of delay; so a proposal that expires
                //   in `e` will not be refunded in the transition from `e` to `e+1` but in the
                //   one from `e+1` to `e+2`.
                //
                // So, putting it all together:
                //
                // 1. A proposal that is valid until `e` must be refunded in the transition
                //   from `e+1` to `e+2`;
                //
                // 2. `epoch` designates the arrival epoch (i.e. `e+2`);
                //
                // Hence: epoch == valid_until + 2
                if epoch == row.valid_until + 2 || pruned_proposals.contains(&key) {
                    let key = expect_stake_credential(&row.proposal.reward_account);
                    refunds
                        .entry(key)
                        // NOTE: There may be *multiple* refunds for the same credential.
                        // So it's important not to simply override the refund value with a
                        // blind 'insert'.
                        .and_modify(|entry| *entry += row.proposal.deposit)
                        .or_insert_with(|| row.proposal.deposit);
                    *item.borrow_mut() = None;
                // While proposals are only refunded in e+2, they aren't 'votable' in 'e+1'; thus
                // they cannot be considered active in e+1.
                } else if epoch <= row.valid_until {
                    still_active += 1;
                }
            }
        }
    })?;

    if still_active == 0 {
        let mut governance_activity = db.governance_activity()?;
        governance_activity.consecutive_dormant_epochs += 1;
        db.set_governance_activity(&governance_activity)?;
    }

    refund_many(db, refunds.into_iter())?;

    Ok(ctx.protocol_parameters)
}

#[observability_trace(amaru::ledger::state::RATIFICATION_CONTEXT_NEW)]
fn new_ratification_context<'distr>(
    snapshot: impl Snapshot,
    stake_distribution: StakeDistributionView<'distr>,
    protocol_parameters: ProtocolParameters,
    treasury: Lovelace,
) -> Result<RatificationContext<'distr>, StoreError> {
    let constitutional_committee = match snapshot.constitutional_committee()? {
        ConstitutionalCommitteeStatus::NoConfidence => None,
        ConstitutionalCommitteeStatus::Trusted { threshold } => {
            let members = snapshot
                .iter_cc_members()?
                .filter_map(|(cold_credential, row)| {
                    row.valid_until
                        .map(|valid_until| (cold_credential, (row.hot_credential, valid_until)))
                })
                .collect();

            Some(ratification::ConstitutionalCommittee::new(
                into_safe_ratio(&threshold),
                members,
            ))
        }
    };

    // FIXME: This isn't ideal , as we collect all votes in memory here. This is okay-ish on most
    // networks because the number of votes is rather small. Even with 1M+ votes, this shouldn't
    // require much memory; but it becomes a potential attack vector.
    //
    // So ideally, we should avoid loading votes in memory.
    let votes = snapshot
        .iter_votes()?
        .fold(BTreeMap::new(), |mut votes, (k, v)| {
            votes
                .entry(k.proposal)
                .or_insert_with(Vec::new)
                .push((k.voter, v));

            votes
        });

    Ok(RatificationContext {
        // Ratification happens with one epoch of delay, and at the next epoch transition. So,
        // if we ratify votes that happened in epoch `e`, the ratification is done during the
        // transition from `e + 1` to `e + 2`; but it is done "as if" it was happening at the
        // beginning of epoch `e + 1`. So, the epoch we consider for DRep mandates and proposal
        // expiry is the one from after the snapshot.
        epoch: snapshot.epoch() + 1,
        treasury,
        stake_distribution,
        protocol_parameters,
        constitutional_committee,
        votes,
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
    pub fn new(
        guard: MutexGuard<'a, VecDeque<StakeDistribution>>,
        epoch: Epoch,
    ) -> Result<Self, StateError> {
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
        let stake_distribution = view
            .iter()
            .find(|s| s.epoch == epoch)
            .ok_or(GetPoolError::StakeDistributionNotAvailable(epoch))?;

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
    #[error("error rolling back to unknown {0:?}")]
    UnknownRollbackPoint(Point),
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
            governance::Error::EraHistoryError(slot, err) => {
                StateError::ErrorComputingEpoch(slot, err)
            }
            governance::Error::StoreError(err) => StateError::Storage(err),
        }
    }
}
