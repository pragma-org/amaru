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
pub mod volatile_db;

use crate::{
    state::volatile_db::{StoreUpdate, VolatileDB},
    store::{HistoricalStores, Store, StoreError, TransactionalContext},
    summary::{
        governance::GovernanceSummary,
        rewards::{RewardsSummary, StakeDistribution},
    },
};
use amaru_kernel::{
    Epoch, EraHistory, Hash, MintedBlock, Point, PoolId, Slot, TransactionInput, TransactionOutput,
    CONSENSUS_SECURITY_PARAM, MAX_KES_EVOLUTION, SLOTS_PER_KES_PERIOD, STABILITY_WINDOW,
};
use amaru_ouroboros_traits::{HasStakeDistribution, PoolSummary};
use slot_arithmetic::TimeHorizonError;
use std::{
    borrow::Cow,
    collections::{BTreeSet, VecDeque},
    sync::{Arc, Mutex},
};
use thiserror::Error;
use tracing::{instrument, trace, Level};
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
///   top of the _stable_ store. It contains at most 'CONSENSUS_SECURITY_PARAM' entries; old entries
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

    /// The era history for the network this store is related to
    era_history: EraHistory,
}

impl<S: Store, HS: HistoricalStores> State<S, HS> {
    #[allow(clippy::unwrap_used)]
    pub fn new(stable: Arc<Mutex<S>>, snapshots: HS, era_history: &EraHistory) -> Self {
        let db = stable.lock().unwrap();

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
        let latest_epoch = db.epoch();

        let mut stake_distributions = VecDeque::new();
        #[allow(clippy::panic)]
        for epoch in latest_epoch - 2..=latest_epoch - 1 {
            stake_distributions.push_front(
                recover_stake_distribution(&snapshots, epoch).unwrap_or_else(|e| {
                    // TODO deal with error
                    panic!(
                        "unable to get stake distribution for (epoch={:?}): {e:?}",
                        epoch
                    )
                }),
            );
        }

        drop(db);

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
            // (2) Re-applying 2160 (already synchronized) blocks is _fast-enough_ that it can be
            //     done on restart easily. To be measured; if this turns out to be too slow, we
            //     views of the volatile DB on-disk to be able to restore them quickly.
            volatile: VolatileDB::default(),

            rewards_summary: None,

            stake_distributions: Arc::new(Mutex::new(stake_distributions)),

            era_history: era_history.clone(),
        }
    }

    /// Obtain a view of the stake distribution, to allow decoupling the ledger from other
    /// components that require access to it.
    pub fn view_stake_distribution(&self) -> impl HasStakeDistribution {
        StakeDistributionView {
            view: self.stake_distributions.clone(),
            era_history: self.era_history.clone(),
        }
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

        // Note: the volatile sequence may contain points belonging to two epochs. We diligently
        // make snapshots at the end of each epoch. Thus, as soon as the next stable block is
        // exactly MORE than one epoch apart, it means that we've already pushed to the stable
        // db all the blocks from the previous epoch and it's time to make a snapshot before we
        // apply this new stable diff.
        //
        // However, 'current_epoch' here refers to the _ongoing_ epoch in the volatile db. So
        // we must snapshot the one _just before_.
        let db = self.stable.lock().unwrap();

        let mut transaction = db.create_transaction();
        if current_epoch > db.epoch() + 1 {
            epoch_transition(&mut transaction, current_epoch, self.rewards_summary.take())?;
        }

        let StoreUpdate {
            point: stable_point,
            issuer: stable_issuer,
            fees,
            add,
            remove,
            withdrawals,
            voting_dreps,
        } = now_stable.into_store_update(current_epoch);
        transaction
            .save(
                &stable_point,
                Some(&stable_issuer),
                add,
                remove,
                withdrawals,
                voting_dreps,
            )
            .and_then(|()| {
                transaction.with_pots(|mut row| {
                    row.borrow_mut().fees += fees;
                })
            })
            .map_err(StateError::Storage)?;
        transaction.commit().map_err(StateError::Storage)
    }

    #[allow(clippy::unwrap_used)]
    #[instrument(level = Level::TRACE, skip_all)]
    fn compute_rewards(&mut self) -> Result<RewardsSummary, StateError> {
        let mut stake_distributions = self.stake_distributions.lock().unwrap();
        let stake_distribution = stake_distributions
            .pop_back()
            .ok_or(StateError::StakeDistributionNotAvailableForRewards)?;

        let epoch = stake_distribution.epoch + 2;

        #[allow(clippy::panic)]
        let rewards_summary = RewardsSummary::new(
            &self.snapshots.for_epoch(epoch).unwrap_or_else(|e| {
                panic!(
                    "unable to open database snapshot for epoch {:?}: {:?}",
                    epoch, e
                )
            }),
            stake_distribution,
        )
        .map_err(StateError::Storage)?;

        stake_distributions.push_front(
            recover_stake_distribution(&self.snapshots, epoch).map_err(StateError::Storage)?,
        );

        Ok(rewards_summary)
    }

    /// Roll the ledger forward with the given block by applying transactions one by one, in
    /// sequence. The update stops at the first invalid transaction, if any. Otherwise, it updates
    /// the internal state of the ledger.
    #[allow(clippy::unwrap_used)]
    #[instrument(level = Level::TRACE, skip_all)]
    pub fn forward(&mut self, next_state: AnchoredVolatileState) -> Result<(), StateError> {
        // Persist the next now-immutable block, which may not quite exist when we just
        // bootstrapped the system
        if self.volatile.len() >= CONSENSUS_SECURITY_PARAM {
            let now_stable = self.volatile.pop_front().unwrap_or_else(|| {
                unreachable!("pre-condition: self.volatile.len() >= CONSENSUS_SECURITY_PARAM")
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

        if self.rewards_summary.is_none() && relative_slot >= From::from(STABILITY_WINDOW as u64) {
            self.rewards_summary = Some(self.compute_rewards()?);
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
    pub fn resolve_inputs<'a>(
        &'_ self,
        ongoing_state: &VolatileState,
        inputs: impl Iterator<Item = &'a TransactionInput>,
    ) -> Result<Vec<(TransactionInput, Option<TransactionOutput>)>, StoreError> {
        let mut result = Vec::new();

        // TODO: perform lookup in batch, and possibly within the same transaction as other
        // required data pre-fetch.
        for input in inputs {
            let output = ongoing_state
                .resolve_input(input)
                .cloned()
                .or_else(|| self.volatile.resolve_input(input).cloned())
                .map(|output| Ok(Some(output)))
                .unwrap_or_else(|| {
                    let db = self.stable.lock().unwrap();
                    db.utxo(input)
                })?;

            result.push((input.clone(), output));
        }

        Ok(result)
    }
}

#[allow(clippy::panic)]
fn recover_stake_distribution(
    snapshots: &impl HistoricalStores,
    epoch: Epoch,
) -> Result<StakeDistribution, StoreError> {
    let snapshot = snapshots.for_epoch(epoch).unwrap_or_else(|e| {
        panic!(
            "unable to open database snapshot for epoch {:?}: {:?}",
            epoch, e
        )
    });

    StakeDistribution::new(&snapshot, GovernanceSummary::new(&snapshot)?)
}

#[instrument(level = Level::TRACE, skip_all)]
fn epoch_transition<'store>(
    transaction: &mut impl TransactionalContext<'store>,
    current_epoch: Epoch,
    rewards_summary: Option<RewardsSummary>,
) -> Result<(), StateError> {
    transaction
        .next_snapshot(current_epoch - 1, rewards_summary)
        .map_err(StateError::Storage)?;
    // Then we, can tick pools to compute their new state at the epoch boundary. Notice
    // how we tick with the _current epoch_ however, but we take the snapshot before
    // the tick since the actions are only effective once the epoch is crossed.
    transaction
        .tick_pools(current_epoch)
        .map_err(StateError::Storage)?;

    // Refund deposit for any proposal that has expired.
    transaction
        .tick_proposals(current_epoch)
        .map_err(StateError::Storage)?;
    Ok(())
}

// HasStakeDistribution
// ----------------------------------------------------------------------------

// The 'LedgerState' trait materializes the interface required of the consensus layer in order to
// validate block headers. It allows to keep the ledger implementation rather abstract to the
// consensus in order to decouple both components.
pub struct StakeDistributionView {
    view: Arc<Mutex<VecDeque<StakeDistribution>>>,
    era_history: EraHistory,
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

    fn slot_to_kes_period(&self, slot: u64) -> u64 {
        slot / SLOTS_PER_KES_PERIOD
    }

    fn max_kes_evolutions(&self) -> u64 {
        MAX_KES_EVOLUTION as u64
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
    #[error("error processing query")]
    Query(#[source] StoreError),
    #[error("unable to open database snapshot for epoch {1}: {0}")]
    EpochAccess(u64, #[source] StoreError),
    #[error("error accessing storage")]
    Storage(#[source] StoreError),
    #[error("no stake distribution available for rewards calculation.")]
    StakeDistributionNotAvailableForRewards,
    #[error("failed to compute epoch from slot {0:?}: {1}")]
    ErrorComputingEpoch(Slot, TimeHorizonError),
}
