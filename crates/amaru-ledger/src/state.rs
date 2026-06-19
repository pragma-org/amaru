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
    collections::VecDeque,
    ops::Deref,
    sync::{Arc, MutexGuard},
};

use amaru_kernel::{
    Block, Epoch, EraHistory, EraHistoryError, GlobalParameters, NetworkName, Point, PoolId, ProtocolParameters, Slot,
    Tip, Transaction, TransactionInput,
};
use amaru_metrics::ledger::LedgerMetrics;
use amaru_observability::info_span;
use amaru_ouroboros_traits::{PoolSummary, pools::GetPoolError};
use amaru_plutus::arena_pool::ArenaPool;
use num::CheckedSub;
use thiserror::Error;
use tracing::warn;

use crate::{
    epoch_transition::GovernanceActivity,
    rules,
    rules::block::BlockValidation,
    state::volatile::{AnchoredVolatileFragment, VolatileDB},
    state_snapshot::StateSnapshot,
    store::{HistoricalStores, Snapshot, Store, StoreError},
    summary::{
        governance::{self, GovernanceSummary},
        stake_distribution::StakeDistribution,
    },
};

pub mod diff_bind;
pub mod diff_epoch_reg;
pub mod diff_set;
pub mod overlay;
pub mod volatile;

/// The minimum number of past (from the current epoch) snapshots required for the ledger to
/// operate.
pub const MIN_LEDGER_SNAPSHOTS: u64 = 3;

pub(crate) const EVENT_TARGET: &str = "amaru::ledger::state";

// State
// ----------------------------------------------------------------------------

/// `State` provides access to the ledger data in a thread-safe mode, considering n readers and one writer:
///
///  - The `load` method returns an `Arc<StateSnapshot>` that allows readers to get a snapshot of the ledger data
///    without being blocked by the writer (in practice there is only one writer, the `validate_block` consensus stage).
///
///  - The writer uses the `transaction` method to mutate a candidate clone of the live `StateSnapshot` and atomically
///    publish it if the modification is successful.
///
pub struct State<S, HS> {
    /// Live view of the ledger. It is wrapped in an `RwLock<Arc<...>>` so that
    /// readers can take a brief read lock and `Arc::clone` the inner pointer, while the writer
    /// takes the write lock long enough to publish the next view.
    current: Arc<parking_lot::RwLock<Arc<StateSnapshot<S, HS>>>>,
}

impl<S: Store, HS: HistoricalStores> Clone for State<S, HS> {
    fn clone(&self) -> Self {
        Self { current: self.current.clone() }
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
        StateSnapshot::new(stable, snapshots, network, era_history, global_parameters).map(Self::from_state_snapshot)
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
        stake_distributions: VecDeque<Arc<StakeDistribution>>,
    ) -> Self {
        Self::from_state_snapshot(StateSnapshot::new_with(
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

    fn from_state_snapshot(snapshot: StateSnapshot<S, HS>) -> Self {
        Self { current: Arc::new(parking_lot::RwLock::new(Arc::new(snapshot))) }
    }

    /// Load a snapshot of the current ledger.
    pub fn load(&self) -> Arc<StateSnapshot<S, HS>> {
        self.current.read().clone()
    }

    /// The `atomically` function is used to apply mutable modifications to a `StateSnapshot`.
    /// If those modifications are successful, as witnessed by the `bool` value, then the new
    /// `StateSnapshot` value is atomically swapped with the previous one.
    ///
    /// Readers can access the ledger state during the closure execution only if that function does
    /// not change the stable state. When `atomically` is used to roll forward a block,
    /// the stable state can be modified if `StateOverlay::is_empty` returns `false` (see [`StateSnapshot::apply_block`]).
    /// In that case we don't want readers to access some partially committed state so we call
    /// `modify_exclusive` to get a full lock on the state.
    pub fn atomically<R>(&self, f: impl FnOnce(&mut StateSnapshot<S, HS>) -> (R, bool)) -> R {
        if self.load().is_empty() { self.modify(f) } else { self.modify_exclusive(f) }
    }

    /// Modify a *candidate* clone of the current ledger state and atomically
    /// publish the result if the modifications is successful. The provided closure returns `(R, bool)`:
    /// - The `R` value is returned to the caller.
    /// - If `bool` is true then the modified state snapshot is swapped with the old version.
    ///   If it is `false` that modified state snapshot is just dropped.
    fn modify<R>(&self, f: impl FnOnce(&mut StateSnapshot<S, HS>) -> (R, bool)) -> R {
        // Clone the live view into a candidate. The read lock is held only long enough
        // to bump the inner Arc's refcount; the clone of `StateSnapshot` itself happens
        // outside the lock because we drop the guard immediately after.
        let mut candidate = (**self.current.read()).clone();
        let (result, publish) = f(&mut candidate);

        if publish {
            *self.current.write() = Arc::new(candidate);
        }
        result
    }

    /// Sibling of [`Self::modify`] that holds the swap `RwLock` for the **entire**
    /// duration of the mutation, blocking `load()` calls until the candidate is published
    /// (or dropped).
    ///
    /// Used for mutations that touch both the in-memory overlay *and* the stable store
    /// in a way that must be observed atomically by readers.
    fn modify_exclusive<R>(&self, f: impl FnOnce(&mut StateSnapshot<S, HS>) -> (R, bool)) -> R {
        let mut guard = self.current.write();
        let mut candidate = (**guard).clone();
        let (result, publish) = f(&mut candidate);

        if publish {
            *guard = Arc::new(candidate);
        }
        result
    }

    pub fn tip(&self) -> Cow<'_, Point> {
        Cow::Owned(self.load().tip().into_owned())
    }

    pub fn volatile_tip(&self) -> Option<Tip> {
        self.load().volatile_tip()
    }

    pub fn immutable_tip(&self) -> Point {
        self.load().immutable_tip()
    }

    pub fn contains_volatile_point(&self, point: &Point) -> bool {
        self.load().contains_volatile_point(point)
    }

    pub fn get_pool_summary(&self, slot: Slot, pool: &PoolId) -> Result<Option<PoolSummary>, GetPoolError> {
        self.load().get_pool_summary(slot, pool)
    }

    pub fn validate_tx(
        &self,
        transaction: &Transaction,
        slot: Slot,
        arena_pool: &ArenaPool,
    ) -> Result<(), rules::block::TransactionValidationFailed> {
        self.load().validate_tx(transaction, slot, arena_pool)
    }

    pub fn registered_relay_socket_addrs(
        &self,
    ) -> Result<std::collections::BTreeSet<std::net::SocketAddr>, StoreError> {
        self.load().registered_relay_socket_addrs()
    }

    pub fn operational_cert_sequence_number(&self, pool_id: &PoolId) -> Result<Option<u64>, StoreError> {
        self.load().operational_cert_sequence_number(pool_id)
    }

    pub fn push_fragment(
        &self,
        state: AnchoredVolatileFragment,
    ) -> Result<Option<AnchoredVolatileFragment>, StateError> {
        // Publish on success; drop the candidate on error so the live view stays
        // consistent with the failed push.
        self.modify(|view| {
            let result = view.push_fragment(state);
            let publish = result.is_ok();
            (result, publish)
        })
    }

    pub fn roll_forward(
        &self,
        point: &Point,
        block: Block,
        arena_pool: &ArenaPool,
    ) -> BlockValidation<LedgerMetrics, anyhow::Error> {
        // Publish only when there are no infrastructure error and if the block was successfully applied
        self.atomically(|state_snapshot| {
            let result = state_snapshot.roll_forward(point, block, arena_pool);
            let publish = matches!(result, BlockValidation::Valid(_));
            (result, publish)
        })
    }

    pub fn rollback_to(&self, to: &Point) -> Result<(), BackwardError> {
        // `StateSnapshot::rollback_to` validates the target before mutating, so on `Err`
        // the candidate is unchanged; dropping it vs publishing it is equivalent. We
        // still drop on error for clarity.
        self.modify(|state_snapshot| {
            let result = state_snapshot.rollback_to(to);
            let publish = result.is_ok();
            (result, publish)
        })
    }
}

#[derive(Debug, Error)]
pub enum ValidationContextError {
    #[error("failed to resolve inputs: {0}")]
    ResolveInputs(#[from] StoreError),

    #[error("missing transaction inputs: {inputs:?}")]
    MissingInputs { inputs: Vec<TransactionInput> },
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
) -> Result<VecDeque<Arc<StakeDistribution>>, StoreError> {
    let mut stake_distributions = VecDeque::new();

    let latest_epoch = snapshots.most_recent_snapshot();
    let epoch_for_leader_schedule = latest_epoch.checked_sub(Epoch::ONE);
    let epoch_for_rewards = latest_epoch.checked_sub(Epoch::TWO);

    for (ix, epoch) in [epoch_for_rewards, epoch_for_leader_schedule, Some(latest_epoch)].into_iter().enumerate() {
        if let Some(epoch) = epoch {
            let snapshot = snapshots.for_epoch(epoch)?;
            stake_distributions.push_front(Arc::new(
                compute_stake_distribution(&snapshot, era_history).map_err(|err| StoreError::Internal(err.into()))?,
            ));
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
    guard: MutexGuard<'a, VecDeque<Arc<StakeDistribution>>>,
    position: usize,
}

impl<'a> StakeDistributionView<'a> {
    pub fn new(guard: MutexGuard<'a, VecDeque<Arc<StakeDistribution>>>, epoch: Epoch) -> Result<Self, StateError> {
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
        self.guard[self.position].as_ref()
    }
}

// NOTE: calculating current epoch from slot on block application.
//
// This is only safe provided the next_tip is within the foreseeable window. If this isn't
// the case, it's a clear signal of something going very wrong in the consensus/networking
// pipeline feeding blocks to the ledger since they'd be attempting to feed a block that is
// many day after the last applied block!
pub(crate) fn unsafe_slot_to_epoch(era_history: &EraHistory, slot: Slot) -> Epoch {
    era_history
        .slot_to_epoch_unchecked_horizon(slot)
        .unwrap_or_else(|e| unreachable!("impossible; failed to compute epoch from tip ({slot:?}): {e:?}"))
}

// See [`unsafe_slot_to_epoch`]
pub(crate) fn unsafe_slot_in_epoch(era_history: &EraHistory, slot: Slot) -> Slot {
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
}

impl From<governance::Error> for StateError {
    fn from(origin: governance::Error) -> Self {
        match origin {
            governance::Error::EraHistoryError(slot, err) => StateError::ErrorComputingEpoch(slot, err),
            governance::Error::StoreError(err) => StateError::Storage(err),
        }
    }
}
