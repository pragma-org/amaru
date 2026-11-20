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

use std::{
    collections::{BTreeMap, VecDeque},
    sync::Arc,
    time::{Duration, Instant},
};

use amaru_kernel::{Epoch, EraHistory, NetworkName, Point, PoolId, utils::string::display_collection};
use amaru_observability::{info, info_span};
use amaru_ouroboros_traits::{PoolSummaries, PoolSummary};
use num::CheckedSub;

use crate::{
    LedgerStateSnapshot, SnapshotFinality, StakeDistribution, StakeSummary,
    state::{
        StateError,
        tasks::background_task::{Cancellable, Runnable},
    },
    store::{HistoricalStores, Snapshot, StoreError},
    summary::governance::GovernanceSummary,
};

/// Compute, during epoch `e`, the stake distribution to be used for epoch `e + 1` using a snapshot
/// of epoch `e - 1`.
pub struct ComputeStakeDistribution<PreviousSnapshot> {
    /// Snapshot of the previous epoch; source of the rotated stake distribution as well as the
    /// block issuers and pots for the rewards calculation.
    previous_epoch_snapshot: PreviousSnapshot,
    /// Set when the in-memory stake distributions do not include the previous epoch yet.
    rotation: Option<PoolSummaries>,
    network: NetworkName,
    era_history: EraHistory,
    on_stake_dist_updated: Option<Arc<dyn Fn(PoolSummaries) + Send + Sync>>,
    on_ledger_snapshot: Option<Arc<dyn Fn(LedgerStateSnapshot<'_>) + Send + Sync>>,
    /// Set when `previous_epoch_snapshot` is a candidate snapshot: the last block of its epoch.
    candidate_last_block: Option<Point>,
}

impl<PreviousSnapshot> ComputeStakeDistribution<PreviousSnapshot> {
    pub fn new(
        previous_epoch_snapshot: PreviousSnapshot,
        rotation: Option<PoolSummaries>,
        network: NetworkName,
        era_history: EraHistory,
        on_stake_dist_updated: Option<Arc<dyn Fn(PoolSummaries) + Send + Sync>>,
        on_ledger_snapshot: Option<Arc<dyn Fn(LedgerStateSnapshot<'_>) + Send + Sync>>,
        candidate_last_block: Option<Point>,
    ) -> Self {
        Self {
            previous_epoch_snapshot,
            rotation,
            network,
            era_history,
            on_stake_dist_updated,
            on_ledger_snapshot,
            candidate_last_block,
        }
    }
}

impl<PreviousSnapshot: Snapshot> Runnable<Option<StakeDistribution>> for ComputeStakeDistribution<PreviousSnapshot> {
    /// Compute the stake distribution from the previous epoch, and notify
    /// consumers (e.g. header validation) with pool summaries covering the refreshed epochs.
    fn run(mut self, cancellable: Cancellable) -> Result<Option<StakeDistribution>, StateError> {
        cancellable.check_cancelled()?;
        let Some(mut summaries) = self.rotation.take() else { return Ok(None) };

        let finality = match self.candidate_last_block {
            Some(last_block) => SnapshotFinality::Candidate { last_block },
            None => SnapshotFinality::Final,
        };
        let distribution = compute_stake_distribution(
            &self.previous_epoch_snapshot,
            self.network,
            &self.era_history,
            self.on_ledger_snapshot.as_deref().map(|notify| (notify, finality)),
            |_| {},
        )?;
        cancellable.check_cancelled()?;

        summaries.by_epoch.extend(pool_summaries_for(std::iter::once(&distribution)).by_epoch);

        info!(
            ledger::stake_distribution::ROTATE,
            available_stake_distributions = display_collection(summaries.by_epoch.keys()),
        );

        if let Some(notify) = &self.on_stake_dist_updated {
            if let Some(last_block) = self.candidate_last_block {
                summaries.candidates.insert(distribution.epoch, last_block);
            }
            notify(summaries);
        }

        Ok(Some(distribution))
    }
}

/// Compute a new stake distribution, retrieved from the new `StakeSummary`.
fn compute_stake_distribution(
    snapshot: &impl Snapshot,
    network: NetworkName,
    era_history: &EraHistory,
    notify_observer: Option<(&(dyn Fn(LedgerStateSnapshot<'_>) + Send + Sync), SnapshotFinality)>,
    notify_progress: impl FnMut(f64),
) -> Result<StakeDistribution, StateError> {
    info_span!(ledger::stake_distribution::COMPUTE, epoch = snapshot.epoch(),).in_scope(|| {
        let summary =
            StakeSummary::new(snapshot, GovernanceSummary::new(snapshot, era_history)?, network, notify_progress)
                .map_err(StateError::Storage)?;

        // Opt-in: show the full summary (incl. accounts) by reference before we only
        // retain the slim in-memory distribution. Observer clones individual fields if needed.
        if let Some((notify, finality)) = notify_observer {
            notify(LedgerStateSnapshot { summary: &summary, finality });
        }

        Ok(summary.stake_distribution)
    })
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
pub fn initialize_stake_distributions<HS>(
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
                    info!(ledger::stake_distribution::INITIAL_PROGRESS, epoch, progress);
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

pub fn pool_summaries_for<'iter>(stake_distributions: impl Iterator<Item = &'iter StakeDistribution>) -> PoolSummaries {
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
    PoolSummaries { by_epoch, ..Default::default() }
}
