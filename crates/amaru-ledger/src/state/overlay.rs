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

use std::{mem, sync::Arc};

use amaru_kernel::{Epoch, ProtocolParameters, ProtocolVersion};
use amaru_observability::info_span;
use tracing::{Span, debug};

use crate::{
    epoch_transition::{
        Computed, Effective, GovernanceActivity, GovernanceUpdates, PoolsEpochTransitionUpdates, Rewards, RewardsState,
    },
    state::StateError,
    store::{
        EpochTransitionProgress, Store, TransactionalContext, apply_governance_updates, pay_or_refund_accounts,
        pay_rewards, reset_blocks_count, reset_fees, update_or_retire_pools,
    },
};

/// Represents the information we sometimes have to overlay on top of the immutable store. That is,
/// they are computed bits of the ledger state that aren't stable yet but that still need to be
/// accounted for for block validation. They are computed at each epoch boundaries, and flushed
/// once we've reached enter the stability window of each epoch.
#[derive(Clone)]
pub struct StateOverlay {
    /// The last known epoch; or said differently, the epoch for which this overlay is valid.
    epoch: Epoch,

    /// The computed rewards summary to be applied on the next epoch boundary. This is computed
    /// once in the epoch, and held until the end where it is reset.
    ///
    /// It also contains the latest stake distribution computed from the previous epoch, which we
    /// hold onto the epoch boundary. In the epoch boundary, the stake distribution becomes
    /// available for the leader schedule verification, whereas the stake distribution previously
    /// used for leader schedule is moved as rewards stake.
    ///
    /// Wrapped in `Arc` so cloning the overlay is cheap. The `Rewards<Computed>` /
    /// `Rewards<Effective>` variants hold a `BTreeMap` of per-account rewards which can reach
    /// ~1M entries on mainnet; only mutations pay the deep-clone cost, via `Arc::make_mut`.
    rewards: Arc<RewardsState>,

    /// Computed pools updates that are pending application to the stable store. The value is only
    /// `Some` during the first `k` blocks of an epoch since this corresponds to the unstable part
    /// of an epoch.
    ///
    /// When present, they must be taken into account when creating the ledger validation context.
    ///
    /// Wrapped in `Arc` so cloning the overlay is cheap. Contains a `BTreeMap<PoolId, Pool>` of
    /// pool params (several MB at peak); only mutations pay the deep-clone cost.
    pools_updates: Option<Arc<PoolsEpochTransitionUpdates>>,

    /// The result of an epoch boundary ratification, stashed temporarily until it is stable enough
    /// to persist in the stable storage.
    governance_updates: Option<GovernanceUpdates>,

    /// Updatable protocol parameters, cached from the stable store.
    protocol_parameters: ProtocolParameters,

    /// Track the number of dormant epochs (i.e. epochs that start without any available proposals).
    governance_activity: GovernanceActivity,
}

impl StateOverlay {
    /// Construct a new default/empty overlay from current parameters.
    pub fn new(epoch: Epoch, protocol_parameters: ProtocolParameters, governance_activity: GovernanceActivity) -> Self {
        Self {
            epoch,
            rewards: Arc::new(RewardsState::NotReady),
            pools_updates: None,
            governance_updates: None,
            protocol_parameters,
            governance_activity,
        }
    }
}

impl StateOverlay {
    /// Rollback an existing overlay, throwing away the epoch transition calculations.
    pub fn rollback(&mut self) {
        let to = self.epoch - 1;
        debug!(name: "overlay.rollback", from = %self.epoch, %to, "overlay.rollback");

        self.epoch = to;
        // Mutate the inner `RewardsState` in place rather than swapping the outer `Arc`. The Arc
        // field is never observably `NotReady` during this call: `make_mut` clones into a fresh
        // Arc if the current one is shared, and the `mem::take`/assign happens behind `&mut RewardsState` that nothing else can observe.
        let rewards = Arc::make_mut(&mut self.rewards);
        *rewards = match mem::take(rewards) {
            st @ RewardsState::NotReady | st @ RewardsState::Computed(..) => st,
            RewardsState::Effective(effective) => RewardsState::Computed(effective.into()),
        };
        self.pools_updates = None;
        self.governance_updates = None;
    }

    /// Record transition into a new epoch.
    pub fn transition(
        &mut self,
        effective_rewards: Option<Rewards<Effective>>,
        pools_updates: PoolsEpochTransitionUpdates,
        governance_updates: GovernanceUpdates,
    ) {
        let to = self.epoch + 1;
        debug!(name: "overlay.transition", from = %self.epoch, %to, "overlay.transition");

        self.epoch = to;
        self.rewards = Arc::new(effective_rewards.map(RewardsState::Effective).unwrap_or(RewardsState::NotReady));
        self.pools_updates = Some(Arc::new(pools_updates));
        self.governance_updates = Some(governance_updates);
    }

    /// Flush an overlay to disk.
    pub fn apply(&mut self, db: &impl Store) -> Result<(), StateError> {
        info_span!(
            amaru_observability::amaru::ledger::epoch_transition::APPLYING_OVERLAY,
            epoch = u64::from(self.epoch)
        )
        .in_scope(|| {
            use EpochTransitionProgress::*;

            // ---------------------------------------------------------------------------- End of epoch
            db.with_transaction::<_, StateError>(|batch| {
                let should_end_epoch = batch.try_epoch_transition(None, Some(EpochEnded))?;

                Span::current().record("should_end_epoch", should_end_epoch);

                // Mutate the inner `RewardsState` in place rather than swapping the outer `Arc`,
                // so the field is never observably `NotReady` mid-call. See `rollback` for the
                // same pattern.
                let rewards = Arc::make_mut(&mut self.rewards);
                if should_end_epoch {
                    let RewardsState::Effective(effective_rewards) = mem::take(rewards) else {
                        return Err(StateError::NoEffectiveRewards);
                    };
                    pay_rewards(batch, effective_rewards)?;
                } else {
                    *rewards = RewardsState::NotReady;
                }

                Ok(())
            })?;

            // ------------------------------------------------------------------------------ Snapshot
            db.with_transaction::<_, StateError>(|batch| {
                let should_snapshot = batch.try_epoch_transition(Some(EpochEnded), Some(SnapshotTaken))?;

                Span::current().record("should_snapshot", should_snapshot);

                if should_snapshot {
                    db.next_snapshot(self.epoch - 1)?;
                }

                Ok(())
            })?;

            // -------------------------------------------------------------------------- Start of epoch
            db.with_transaction::<_, StateError>(|batch| {
                let should_begin_epoch = batch.try_epoch_transition(Some(SnapshotTaken), Some(EpochStarted))?;

                Span::current().record("should_begin_epoch", should_begin_epoch);

                if should_begin_epoch {
                    reset_blocks_count(batch)?;

                    reset_fees(batch)?;

                    if let Some(pools_updates) = mem::take(&mut self.pools_updates) {
                        let mut pools_updates = Arc::unwrap_or_clone(pools_updates);
                        update_or_retire_pools(batch, pools_updates.take_updated(), pools_updates.take_retired())?;
                        pay_or_refund_accounts(batch, pools_updates.refunds())?;
                    } else {
                        debug!(name: "overlay.no_pools_updates", "overlay.no_pools_updates");
                    }

                    if let Some(governance_updates) = mem::take(&mut self.governance_updates) {
                        let (protocol_parameters, governance_activity) =
                            apply_governance_updates(batch, governance_updates)?;
                        self.protocol_parameters = protocol_parameters;
                        self.governance_activity = governance_activity;
                    } else {
                        debug!(name: "overlay.no_governance_updates", "overlay.no_governance_updates");
                    }
                } else {
                    mem::take(&mut self.pools_updates);
                    mem::take(&mut self.governance_updates);
                }

                Ok(())
            })
        })?;

        assert!(matches!(*self.rewards, RewardsState::NotReady), "rewards leftovers after flushing overlay?");
        assert!(self.governance_updates.is_none(), "governance updates leftovers after flushing overlay?");
        assert!(self.pools_updates.is_none(), "pools updates leftovers after flushing overlay?");

        Ok(())
    }
}

impl StateOverlay {
    /// Check whether the overlay has unapplied state
    pub fn is_empty(&self) -> bool {
        matches!(*self.rewards, RewardsState::NotReady | RewardsState::Computed(..))
            && self.pools_updates.is_none()
            && self.governance_updates.is_none()
    }

    /// The last known epoch; or said differently, the epoch for which this overlay is valid.
    pub fn epoch(&self) -> Epoch {
        self.epoch
    }

    /// Get current protocol version, applying the overlay if necessary.
    pub fn protocol_version(&self) -> ProtocolVersion {
        let (major, minor) = self.protocol_parameters().protocol_version;
        (major, minor)
    }

    /// Obtain the protocol parameters for a specific epoch; which can either be the *current*
    /// epoch as per the latest tip; or the previous one. This is useful when applying the last
    /// `k` blocks of an epoch.
    ///
    /// At this point, the tip has already transitioned, but we still need some of the protocol
    /// parameters *at the time of that block* during persistence; mostly because of branching
    /// logic that depends on protocol version.
    pub fn protocol_parameters_for(&self, epoch: Epoch) -> &ProtocolParameters {
        if epoch == self.epoch {
            self.protocol_parameters()
        } else {
            self.assert_previous_epoch(epoch);
            &self.protocol_parameters
        }
    }

    /// Obtain the latest protocol parameters, from the overlay if any.
    pub fn protocol_parameters(&self) -> &ProtocolParameters {
        self.governance_updates.as_ref().map(|update| &update.protocol_parameters).unwrap_or(&self.protocol_parameters)
    }

    /// Similar to [`Self::protocol_parameters_for`], we need to hold onto the governance activity at the
    /// time of a block, and not the value at the tip (since we apply block with ~2160 blocks of
    /// delays.
    pub fn governance_activity_for(&self, epoch: Epoch) -> GovernanceActivity {
        if epoch == self.epoch {
            self.governance_activity()
        } else {
            self.assert_previous_epoch(epoch);
            self.governance_activity
        }
    }

    /// Obtain the latest governance activity, from the overlay if any.
    pub fn governance_activity(&self) -> GovernanceActivity {
        let mut governance_activity = self.governance_activity;

        if self.governance_updates.as_ref().is_some_and(|updates| updates.is_dormant_epoch) {
            governance_activity.consecutive_dormant_epochs += 1;
        }

        governance_activity
    }

    /// Obtain a mutable reference to the governance activity, for updating after a block
    /// application.
    pub fn governance_activity_mut(&mut self) -> &mut GovernanceActivity {
        &mut self.governance_activity
    }

    /// A read-only handle on the rewards state.
    pub fn rewards(&self) -> &RewardsState {
        &self.rewards
    }

    /// A mut handle on the rewards state. Use with care to replace rewards.
    ///
    /// `Arc::make_mut` clones the inner `RewardsState` on first mutation if the `Arc` is shared
    /// with a reader.
    pub fn rewards_mut(&mut self) -> &mut RewardsState {
        debug_assert_eq!(
            Arc::strong_count(&self.rewards),
            1,
            "rewards_mut called while Arc<RewardsState> has multiple holders"
        );
        Arc::make_mut(&mut self.rewards)
    }

    /// Consume a computed summary from a previous computation and mark the rewards as 'NotReady'.
    pub fn take_computed_rewards(&mut self) -> Option<Rewards<Computed>> {
        debug_assert_eq!(
            Arc::strong_count(&self.rewards),
            1,
            "take_computed_rewards called while Arc<RewardsState> has multiple holders"
        );
        Arc::make_mut(&mut self.rewards).take_computed_rewards()
    }

    fn assert_previous_epoch(&self, epoch: Epoch) {
        assert!(
            epoch + 1 == self.epoch,
            "invariant violation: asking protocol parameters for an epoch ({}) that's neither current ({}) nor the precedent ({})",
            epoch,
            self.epoch,
            self.epoch.saturating_sub(1),
        );
    }
}
