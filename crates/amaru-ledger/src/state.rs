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

pub mod diff_bind;
pub mod diff_epoch_reg;
pub mod diff_set;
pub mod snapshot;
pub mod volatile_db;

use crate::{
    state::volatile_db::{StoreUpdate, VolatileDB},
    store::{
        columns::pools, EpochTransitionProgress, HistoricalStores, Snapshot, Store, StoreError,
        TransactionalContext,
    },
    summary::{
        governance::{self, GovernanceSummary},
        rewards::RewardsSummary,
        stake_distribution::StakeDistribution,
    },
};
use amaru_kernel::{
    expect_stake_credential,
    protocol_parameters::{GlobalParameters, ProtocolParameters},
    stake_credential_hash, stake_credential_type, EraHistory, Hash, Lovelace, MintedBlock, Point,
    PoolId, ProtocolVersion, Slot, StakeCredential, TransactionInput, TransactionOutput,
    PROTOCOL_VERSION_9,
};
use amaru_ouroboros_traits::{HasStakeDistribution, PoolSummary};
use slot_arithmetic::{Epoch, TimeHorizonError};
use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{Arc, Mutex},
};
use thiserror::Error;
use tracing::{debug, instrument, trace, Level};
use volatile_db::AnchoredVolatileState;

pub use volatile_db::VolatileState;

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

    global_parameters: Arc<GlobalParameters>,

    protocol_parameters: Arc<ProtocolParameters>,
}

impl<S: Store, HS: HistoricalStores> State<S, HS> {
    pub fn new(
        stable: S,
        snapshots: HS,
        era_history: EraHistory,
        global_parameters: GlobalParameters,
    ) -> Result<Self, StoreError> {
        let stake_distributions =
            initial_stake_distributions(&stable, &snapshots, &era_history, PROTOCOL_VERSION_9)?; // FIXME ProtocolVersion should be retrieved from the store

        let protocol_parameters =
            stable.get_protocol_parameters_for(&stable.most_recent_snapshot())?;

        Ok(Self::new_with(
            stable,
            snapshots,
            era_history,
            global_parameters,
            protocol_parameters,
            stake_distributions,
        ))
    }

    pub fn new_with(
        stable: S,
        snapshots: HS,
        era_history: EraHistory,
        global_parameters: GlobalParameters,
        protocol_parameters: ProtocolParameters,
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

            protocol_parameters: Arc::new(protocol_parameters),
        }
    }

    /// Obtain a view of the stake distribution, to allow decoupling the ledger from other
    /// components that require access to it.
    pub fn view_stake_distribution(&self) -> impl HasStakeDistribution {
        StakeDistributionView {
            view: self.stake_distributions.clone(),
            era_history: self.era_history.clone(),
            global_parameters: self.global_parameters.clone(),
        }
    }

    pub fn current_epoch(&self, slot: Slot) -> Result<Epoch, StateError> {
        self.era_history
            .slot_to_epoch(slot)
            .map_err(|e| StateError::ErrorComputingEpoch(slot, e))
    }

    pub fn protocol_parameters(&self) -> &ProtocolParameters {
        &self.protocol_parameters
    }

    /// Inspect the tip of this ledger state. This corresponds to the point of the latest block
    /// applied to the ledger.
    #[allow(clippy::panic)]
    #[allow(clippy::unwrap_used)]
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

    #[allow(clippy::unwrap_used)]
    #[instrument(level = Level::TRACE, skip_all, fields(point.slot = ?now_stable.anchor.0.slot_or_default()))]
    fn apply_block(&mut self, now_stable: AnchoredVolatileState) -> Result<(), StateError> {
        let start_slot = now_stable.anchor.0.slot_or_default();

        let current_epoch = self
            .era_history
            .slot_to_epoch(start_slot)
            .map_err(|e| StateError::ErrorComputingEpoch(start_slot, e))?;

        let mut db = self.stable.lock().unwrap();

        let tip = db.tip().map_err(StateError::Storage)?;

        let tip_epoch = self
            .era_history
            .slot_to_epoch(tip.slot_or_default())
            .map_err(|e| StateError::ErrorComputingEpoch(tip.slot_or_default(), e))?;

        let epoch_transitioning = current_epoch > tip_epoch;

        // The volatile sequence may contain points belonging to two epochs.
        //
        // We cross an epoch boundary as soon as the 'now_stable' block belongs to a different
        // epoch than the previously applied block (i.e. the tip of the stable storage).
        if epoch_transitioning {
            epoch_transition(
                &mut *db,
                current_epoch,
                self.rewards_summary.take(),
                &self.protocol_parameters,
            )?
        }

        // Persist changes for this block
        let StoreUpdate {
            point: stable_point,
            issuer: stable_issuer,
            fees,
            add,
            remove,
            withdrawals,
            voting_dreps,
        } = now_stable.into_store_update(current_epoch, &self.protocol_parameters);

        let batch = db.create_transaction();

        batch
            .save(
                &stable_point,
                Some(&stable_issuer),
                add,
                remove,
                withdrawals,
                voting_dreps,
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

    #[allow(clippy::unwrap_used)]
    #[instrument(level = Level::TRACE, skip_all)]
    fn compute_rewards(
        &mut self,
        protocol_version: ProtocolVersion,
    ) -> Result<RewardsSummary, StateError> {
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

        stake_distributions.push_front(recover_stake_distribution(
            &snapshot,
            &self.era_history,
            protocol_version,
            &self.protocol_parameters,
        )?);

        Ok(rewards_summary)
    }

    /// Roll the ledger forward with the given block by applying transactions one by one, in
    /// sequence. The update stops at the first invalid transaction, if any. Otherwise, it updates
    /// the internal state of the ledger.
    #[allow(clippy::unwrap_used)]
    #[instrument(level = Level::TRACE, skip_all)]
    pub fn forward(
        &mut self,
        protocol_version: ProtocolVersion,
        next_state: AnchoredVolatileState,
    ) -> Result<(), StateError> {
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

        // Once we reach the stability window, compute rewards unless we've already done so.
        let next_state_slot = next_state.anchor.0.slot_or_default();
        let relative_slot = self
            .era_history
            .slot_in_epoch(next_state_slot)
            .map_err(|e| StateError::ErrorComputingEpoch(next_state_slot, e))?;

        if self.rewards_summary.is_none()
            && relative_slot >= Slot::from(self.global_parameters.stability_window as u64)
        {
            self.rewards_summary = Some(self.compute_rewards(protocol_version)?);
        }

        self.volatile.push_back(next_state);

        Ok(())
    }

    pub fn backward(&mut self, to: &Point) -> Result<(), BackwardError> {
        // NOTE: This happens typically on start-up; The consensus layer will typically ask us to
        // rollback to the last known point, which ought to be the tip of the database.
        if self.volatile.is_empty() && self.tip().as_ref() == to {
            return Ok(());
        }

        self.volatile.rollback_to(to, |point| {
            BackwardError::UnknownRollbackPoint(point.clone())
        })
    }

    #[allow(clippy::unwrap_used)]
    #[instrument(level = Level::TRACE, skip_all, name="state.resolve_inputs", fields(resolved_from_context, resolved_from_volatile, resolved_from_db))]
    pub fn resolve_inputs<'a>(
        &'_ self,
        ongoing_state: &VolatileState,
        inputs: impl Iterator<Item = &'a TransactionInput>,
    ) -> Result<Vec<(TransactionInput, Option<TransactionOutput>)>, StoreError> {
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
    db: &impl Store,
    snapshots: &impl HistoricalStores,
    era_history: &EraHistory,
    protocol_version: ProtocolVersion,
) -> Result<VecDeque<StakeDistribution>, StoreError> {
    let latest_epoch = db.most_recent_snapshot();

    let mut stake_distributions = VecDeque::new();
    for epoch in latest_epoch - 2..=latest_epoch - 1 {
        // Retrieve the protocol parameters for the considered epoch
        let protocol_parameters = db.get_protocol_parameters_for(&epoch)?;
        let snapshot = snapshots.for_epoch(epoch)?;
        stake_distributions.push_front(
            recover_stake_distribution(
                &snapshot,
                era_history,
                protocol_version,
                &protocol_parameters,
            )
            .map_err(|err| StoreError::Internal(err.into()))?,
        );
    }

    Ok(stake_distributions)
}

pub fn recover_stake_distribution(
    snapshot: &impl Snapshot,
    era_history: &EraHistory,
    protocol_version: ProtocolVersion,
    protocol_parameters: &ProtocolParameters,
) -> Result<StakeDistribution, StateError> {
    StakeDistribution::new(
        snapshot,
        protocol_version,
        GovernanceSummary::new(snapshot, protocol_version, era_history, protocol_parameters)?,
        protocol_parameters,
    )
    .map_err(StateError::Storage)
}

// Epoch Transitions
// ----------------------------------------------------------------------------

#[instrument(level = Level::INFO, skip_all, fields(from = %next_epoch - 1, into = %next_epoch))]
fn epoch_transition(
    db: &mut impl Store,
    next_epoch: Epoch,
    rewards_summary: Option<RewardsSummary>,
    protocol_parameters: &ProtocolParameters,
) -> Result<(), StateError> {
    // End of epoch
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

    // Snapshot
    let batch = db.create_transaction();
    let should_snapshot = batch.try_epoch_transition(
        Some(EpochTransitionProgress::EpochEnded),
        Some(EpochTransitionProgress::SnapshotTaken),
    )?;
    if should_snapshot {
        db.next_snapshot(next_epoch - 1)?;
    }
    batch.commit()?;

    // Start of epoch
    let batch = db.create_transaction();
    let should_begin_epoch = batch.try_epoch_transition(
        Some(EpochTransitionProgress::SnapshotTaken),
        Some(EpochTransitionProgress::EpochStarted),
    )?;
    if should_begin_epoch {
        begin_epoch(&batch, next_epoch, protocol_parameters)?;
    }
    batch.commit()?;

    Ok(())
}

#[instrument(level = Level::INFO, skip_all)]
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
                if rewards > 0 {
                    if let Some(account) = row.borrow_mut() {
                        account.rewards += rewards;
                    }
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

#[instrument(level = Level::INFO, skip_all)]
fn begin_epoch<'store>(
    db: &impl TransactionalContext<'store>,
    current_epoch: Epoch,
    protocol_parameters: &ProtocolParameters,
) -> Result<(), StoreError> {
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
    tick_pools(db, current_epoch, protocol_parameters)?;

    // Refund deposit for any proposal that has expired.
    tick_proposals(db, current_epoch)?;

    Ok(())
}

// Operation on the state
// ----------------------------------------------------------------------------

#[instrument(
    level = Level::TRACE,
    target = EVENT_TARGET,
    name = "reset.fees",
    skip_all,
)]
pub fn reset_fees<'store>(db: &impl TransactionalContext<'store>) -> Result<(), StoreError> {
    db.with_pots(|mut row| {
        row.borrow_mut().fees = 0;
    })
}

#[instrument(
    level = Level::TRACE,
    target = EVENT_TARGET,
    name = "reset.blocks_count",
    skip_all,
)]
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
) -> Result<(), StoreError> {
    let leftovers =
        refunds.try_fold::<_, _, Result<_, StoreError>>(0, |leftovers, (account, deposit)| {
            debug!(
                target: EVENT_TARGET,
                type = %stake_credential_type(&account),
                account = %stake_credential_hash(&account),
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

#[instrument(level = Level::INFO, name = "tick.pool", skip_all)]
pub fn tick_pools<'store>(
    db: &impl TransactionalContext<'store>,
    epoch: Epoch,
    protocol_parameters: &ProtocolParameters,
) -> Result<(), StoreError> {
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

#[instrument(level = Level::INFO, name = "tick.proposals", skip_all)]
pub fn tick_proposals<'store>(
    db: &impl TransactionalContext<'store>,
    epoch: Epoch,
) -> Result<(), StoreError> {
    let mut refunds: BTreeMap<StakeCredential, Lovelace> = BTreeMap::new();

    db.with_proposals(|iterator| {
        for (_key, item) in iterator {
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
                if epoch == row.valid_until + 2 {
                    refunds.insert(
                        expect_stake_credential(&row.proposal.reward_account),
                        row.proposal.deposit,
                    );
                }
            }
        }
    })?;

    refund_many(db, refunds.into_iter())
}

// HasStakeDistribution
// ----------------------------------------------------------------------------

// The 'LedgerState' trait materializes the interface required of the consensus layer in order to
// validate block headers. It allows to keep the ledger implementation rather abstract to the
// consensus in order to decouple both components.
pub struct StakeDistributionView {
    view: Arc<Mutex<VecDeque<StakeDistribution>>>,
    era_history: Arc<EraHistory>,
    global_parameters: Arc<GlobalParameters>,
}

impl HasStakeDistribution for StakeDistributionView {
    #[allow(clippy::unwrap_used)]
    fn get_pool(&self, slot: Slot, pool: &PoolId) -> Option<PoolSummary> {
        let view = self.view.lock().unwrap();
        let epoch = self.era_history.slot_to_epoch(slot).ok()? - 2;
        view.iter().find(|s| s.epoch == epoch).and_then(|s| {
            s.pools.get(pool).map(|st| PoolSummary {
                vrf: st.parameters.vrf,
                stake: st.stake,
                active_stake: s.active_stake,
            })
        })
    }

    fn slot_to_kes_period(&self, slot: Slot) -> u64 {
        u64::from(slot) / self.global_parameters.slots_per_kes_period
    }

    fn max_kes_evolutions(&self) -> u64 {
        self.global_parameters.max_kes_evolution as u64
    }

    fn latest_opcert_sequence_number(&self, _issuer_vkey: &Hash<28>) -> Option<u64> {
        // FIXME: Move this responsibility to the consensus layer
        None
    }
}

// FailedTransactions
// ----------------------------------------------------------------------------

/// Failed transactions aren'y immediately available in blocks. Only indices of those transactions
/// are stored. This internal structure provides a clean(er) interface to accessing those indices.
pub(crate) struct FailedTransactions {
    inner: BTreeSet<u32>,
}

impl FailedTransactions {
    pub(crate) fn from_block(block: &MintedBlock<'_>) -> Self {
        FailedTransactions {
            inner: block
                .invalid_transactions
                .as_deref()
                .map(|indices| {
                    let mut tree = BTreeSet::new();
                    tree.extend(indices.to_vec().as_slice());
                    tree
                })
                .unwrap_or_default(),
        }
    }

    pub(crate) fn has(&self, ix: u32) -> bool {
        self.inner.contains(&ix)
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
    #[error("rewards summary not ready")]
    RewardsSummaryNotReady,
    #[error("failed to compute epoch from slot {0:?}: {1}")]
    ErrorComputingEpoch(Slot, TimeHorizonError),
}

impl From<governance::Error> for StateError {
    fn from(origin: governance::Error) -> Self {
        match origin {
            governance::Error::TimeHorizonError(slot, err) => {
                StateError::ErrorComputingEpoch(slot, err)
            }
            governance::Error::StoreError(err) => StateError::Storage(err),
        }
    }
}
