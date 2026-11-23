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

use std::{collections::VecDeque, fmt, sync::Arc};

use amaru_kernel::{Epoch, EraHistory, GlobalParameters, NetworkName, Point, ProtocolParameters};
use amaru_observability::{info, warn};
use amaru_ouroboros_traits::PoolSummaries;

use crate::{
    observers::LedgerObservers,
    state::{
        StateError,
        tasks::{
            background_task::BackgroundTask,
            compute_rewards::ComputeRewards,
            compute_stake_distribution::{ComputeStakeDistribution, pool_summaries_for},
        },
    },
    store::{HistoricalStores, Store},
    summary::{rewards::RewardsSummary, stake_distribution::StakeDistribution},
};

/// The most recent snapshot of the ledger, final or candidate, and the stake distribution and
/// rewards computation that reads it.
///
/// The computation of rewards and stake distribution for epoch `e - 1` starts 3·k/f slots into
/// epoch `e`, and the boundary from `e` to `e + 1` joins it.
///
/// A rollback back into epoch `e - 1` cancels it: the replay of the boundary into `e` must not join
/// a computation that is due one epoch later.
///
/// ```text
///   healthy chain:                         finalize
///     ┌───────────────────────────────────────────────────────────────────────────▶ Final(e)
///     │
///   Final(e - 1)
///     │    ▲
///     │    │  roll_back (past the last block), recover
///     │    └────────────────────────┐
///     │                             │
///     └──────────────────────▶ Candidate(e) ────────▶ Settled(e) ─────────────────▶ Final(e)
///   sparse chain: set_as_candidate       finalize               release_settled
/// ```
pub(super) struct LatestSnapshot {
    network: NetworkName,
    global_parameters: Arc<GlobalParameters>,
    era_history: Arc<EraHistory>,
    kind: Kind,
    tasks: BackgroundTask<(Option<StakeDistribution>, RewardsSummary)>,
    /// Whether the stake distribution and rewards computation of the current epoch is done.
    computed: bool,
    /// List of stake distributions up to the latest snapshot, with the most recent first.
    stake_distributions: VecDeque<StakeDistribution>,
    /// This callback is used for headers validation. It receives the pool summaries of
    /// `stake_distributions` each time they change.
    on_stake_dist_updated: Option<Arc<dyn Fn(PoolSummaries) + Send + Sync>>,
}

/// Only the state of the snapshot and of its tasks: the parameters never change.
impl fmt::Debug for LatestSnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LatestSnapshot")
            .field("kind", &self.kind)
            .field("tasks", &self.tasks)
            .field("computed", &self.computed)
            .finish()
    }
}

impl LatestSnapshot {
    /// Create the final snapshot of `epoch`.
    pub fn new(
        epoch: Epoch,
        network: NetworkName,
        global_parameters: Arc<GlobalParameters>,
        era_history: Arc<EraHistory>,
        stake_distributions: VecDeque<StakeDistribution>,
    ) -> Self {
        Self {
            kind: Kind::Final(epoch),
            tasks: BackgroundTask::new("stake distribution and rewards"),
            computed: false,
            network,
            global_parameters,
            era_history,
            stake_distributions,
            on_stake_dist_updated: None,
        }
    }

    /// Set the callback that receives the pool summaries each time the stake distributions change.
    pub fn set_on_stake_dist_updated(&mut self, on_stake_dist_updated: Arc<dyn Fn(PoolSummaries) + Send + Sync>) {
        self.on_stake_dist_updated = Some(on_stake_dist_updated);
    }

    /// Publish the stake distributions held in memory, after a computation published one that is
    /// no longer valid, or with a candidate tag that no longer applies.
    fn publish_stake_distributions(&self) {
        if let Some(notify) = &self.on_stake_dist_updated {
            notify(self.pool_summaries());
        }
    }

    /// The stake distribution of `epoch`, if held in memory.
    pub fn stake_distribution(&self, epoch: Epoch) -> Result<&StakeDistribution, StateError> {
        self.stake_distributions
            .iter()
            .find(|distribution| distribution.epoch == epoch)
            .ok_or(StateError::NoSuitableStakeDistribution(epoch))
    }

    /// The pool summaries of the stake distributions held in memory, for header validation and
    /// the leader schedule.
    pub fn pool_summaries(&self) -> PoolSummaries {
        pool_summaries_for(self.stake_distributions.iter())
    }

    /// The epoch of the most recent snapshot.
    pub fn epoch(&self) -> Epoch {
        match &self.kind {
            Kind::Final(epoch) => *epoch,
            Kind::Candidate(snapshot) | Kind::Settled(snapshot) => snapshot.epoch,
        }
    }

    /// True when the most recent snapshot is final and has no candidate directory on disk.
    pub fn is_final(&self) -> bool {
        matches!(self.kind, Kind::Final(_))
    }

    /// The epoch and the last block of the candidate snapshot if the current snapshot is a candidate one.
    pub fn candidate(&self) -> Option<(Epoch, Point)> {
        self.candidate_snapshot().map(|snapshot| (snapshot.epoch, snapshot.last_block))
    }

    /// The epoch and the last block of the snapshot that still has a directory on disk, candidate or settled.
    #[cfg(any(test, feature = "test-utils"))]
    pub fn candidate_on_disk(&self) -> Option<(Epoch, Point)> {
        self.on_disk().map(|snapshot| (snapshot.epoch, snapshot.last_block))
    }

    /// The last block of the candidate snapshot.
    pub fn candidate_last_block(&self) -> Option<Point> {
        self.candidate_snapshot().map(|snapshot| snapshot.last_block)
    }

    /// Record a freshly rotated stake distribution, keeping only the two most recent ones.
    /// Skipped when the distribution is already held, which can happen after a fork switch
    /// re-crosses the epoch boundary.
    fn install_stake_distribution(&mut self, distribution: StakeDistribution) {
        if self.stake_distributions.front().is_none_or(|front| front.epoch < distribution.epoch) {
            self.stake_distributions.push_front(distribution);
            self.stake_distributions.truncate(2);
        }
    }
}

/// LatestSnapshot tasks management
impl LatestSnapshot {
    /// Start the stake distribution and rewards computation of the epoch after this snapshot in a
    /// background thread, unless it runs or an epoch boundary already joined it. The computation reads this snapshot, and the
    /// snapshot two epochs before it for the stake that the rewards are shared on. It rotates the
    /// stake distributions unless the ledger already holds the distribution of this snapshot, after
    /// a restart.
    pub fn spawn_stake_distribution_and_rewards_task(
        &mut self,
        snapshots: &impl HistoricalStores,
        protocol_parameters: &ProtocolParameters,
        observers: &LedgerObservers,
    ) -> Result<(), StateError> {
        if self.tasks.is_running() || self.computed {
            return Ok(());
        }

        let latest_distribution = self.stake_distributions.front();
        let rotation = if latest_distribution.is_some_and(|latest| latest.epoch >= self.epoch()) {
            None
        } else {
            Some(pool_summaries_for(latest_distribution.into_iter()))
        };
        let stake_distribution = ComputeStakeDistribution::new(
            self.access(snapshots)?,
            rotation,
            self.network,
            (*self.era_history).clone(),
            self.on_stake_dist_updated.clone(),
            observers.on_ledger_snapshot.clone(),
            self.candidate_last_block(),
        );
        let rewards = ComputeRewards::new(
            self.current_epoch(),
            self.access(snapshots)?,
            snapshots.for_epoch(self.epoch() - 2)?,
            self.network,
            (*self.global_parameters).clone(),
            protocol_parameters.clone(),
            (*self.era_history).clone(),
        );
        self.tasks.start(self.current_epoch(), (stake_distribution, rewards));
        Ok(())
    }

    /// Wait for the computation of `epoch`, at the boundary out of that epoch. Installs the rotated
    /// stake distribution, and returns the computed rewards, if that computation ran.
    ///
    /// After this boundary, the snapshot that the computation read is final in all cases. A settled
    /// snapshot is released here. A candidate snapshot is finalized by the flush that the boundary
    /// forces next, and is released at once.
    pub fn join_task(&mut self, epoch: Epoch, store: &impl Store) -> Result<Option<RewardsSummary>, StateError> {
        let result = self.tasks.join(epoch)?;
        if result.is_some() {
            self.computed = true;
        }
        let rewards = result.map(|(stake_distribution, rewards)| {
            if let Some(distribution) = stake_distribution {
                self.install_stake_distribution(distribution);
            }
            rewards
        });
        self.release_settled(store);
        Ok(rewards)
    }

    /// Stop the computation and wait for it.
    pub fn cancel_tasks(&mut self) {
        self.tasks.cancel();
    }
}

/// LatestSnapshot lifecycle
impl LatestSnapshot {
    /// From a final snapshot at epoch `e - 1`, now set the latest ledger snapshot
    /// as a candidate snapshot for epoch `e`
    pub fn set_as_candidate(&mut self, epoch: Epoch, last_block: Point) {
        debug_assert!(
            matches!(self.kind, Kind::Final(final_epoch) if final_epoch + 1 == epoch),
            "a candidate snapshot of epoch {epoch} follows the final snapshot of the epoch before: {self:?}"
        );
        debug_assert!(!self.tasks.is_running(), "no computation runs at the creation of a snapshot: {self:?}");
        let candidate = CandidateSnapshot { epoch, last_block };
        candidate.trace(Outcome::Created);
        self.kind = Kind::Candidate(candidate);
        self.computed = false;
    }

    /// Take the final snapshot of `epoch`, once the stable store holds it:
    ///
    ///  - Final(epoch) -> Final(epoch): the transition out of `epoch` resumes after a restart, and
    ///    the stable store took the snapshot of `epoch` before that restart.
    ///  - Final(epoch - 1) -> Final(epoch): the new final snapshot becomes the latest one.
    ///  - Candidate(epoch) -> Settled(epoch) ?-> Final(epoch): the candidate is final now, but its directory
    ///    stays on disk while the computation reads it, and goes to final if the computation is done.
    ///  - Settled(epoch) ?-> Final(epoch): the candidate is settled now, but its directory
    ///    stays on disk while the computation reads it, and goes to final if the computation is done.
    pub fn finalize(&mut self, epoch: Epoch, store: &impl Store) {
        match &self.kind {
            Kind::Final(final_epoch) if *final_epoch == epoch => {}
            Kind::Final(_) => {
                debug_assert!(!self.tasks.is_running(), "no computation runs at the finalization of {epoch}: {self:?}");
                self.kind = Kind::Final(epoch);
                self.computed = false;
            }
            Kind::Candidate(snapshot) => {
                debug_assert_eq!(snapshot.epoch, epoch, "only the candidate snapshot of {epoch} can be finalized");
                snapshot.trace(Outcome::Final);
                self.kind = Kind::Settled(snapshot.clone());
                self.release_settled(store);
            }
            Kind::Settled(snapshot) => {
                debug_assert_eq!(snapshot.epoch, epoch, "the settled snapshot is the final snapshot of {epoch}");
                self.release_settled(store);
            }
        }
    }

    /// Handle a rollback to `volatile_epoch`, when it goes back into the epoch of a candidate snapshot.
    /// `keeps_last_block` tells if the volatile state still holds the last block of that snapshot.
    ///
    /// Candidate(epoch) -> Final(epoch - 1) when the rollback removed the last block of the snapshot:
    /// the snapshot no longer matches the chain. The returned [`BeforeRollback`] owns the discarded
    /// snapshot, whose directory stays on disk until the fork switch ends with [`Self::commit`] or
    /// [`Self::recover`], so that a failed switch can restore it. A rollback to that last block keeps
    /// the snapshot: a fork from that block has the same end of epoch.
    ///
    /// A settled snapshot stays: the last block of its epoch is stable, so no rollback removes it.
    ///
    /// In both cases, the stake distribution and rewards computation is cancelled, and the stake
    /// distributions in memory are published again.
    pub fn roll_back(&mut self, volatile_epoch: Epoch, keeps_last_block: bool) -> BeforeRollback {
        let before = |candidate| BeforeRollback { candidate, computed: self.computed };
        let Some(snapshot) = self.candidate_snapshot() else {
            return before(Before::NoCandidate);
        };
        if volatile_epoch > snapshot.epoch {
            return before(Before::Kept);
        }
        if keeps_last_block {
            let before = before(Before::Kept);
            self.tasks.cancel();
            self.publish_stake_distributions();
            return before;
        }
        let discarded = snapshot.clone();
        let before = before(Before::Discarded(discarded.clone()));
        self.tasks.cancel();
        self.kind = Kind::Final(discarded.epoch - 1);
        // The rollback crossed the boundary into the epoch of the discarded snapshot. That boundary
        // joined the computation of the snapshot before it, and the ledger still holds its result.
        self.computed = true;
        discarded.trace(Outcome::Discarded);
        self.publish_stake_distributions();
        before
    }

    /// End a fork switch that keeps the new chain: delete the directory of the snapshot that the
    /// rollback discarded, unless the replay took the same snapshot again.
    pub fn commit(&self, before: BeforeRollback, store: &impl Store) {
        if let Before::Discarded(discarded) = before.candidate
            && self.on_disk() != Some(&discarded)
        {
            discarded.remove(store);
        }
    }

    /// End a failed fork switch, once the volatile state of the old chain is back: put back the
    /// snapshot that the rollback discarded, and delete the snapshot that the replay took, if any.
    /// The next header may need the stake distributions of the old chain, so publish them again
    /// after a change of snapshot. A computation that reads an unchanged snapshot keeps running.
    pub fn recover(&mut self, before: BeforeRollback, store: &impl Store) {
        let replayed = self.recover_snapshot(before.candidate);
        self.computed = before.computed;
        if let Some(replayed) = replayed {
            replayed.remove(store);
            self.publish_stake_distributions();
        }
    }

    /// Put back the snapshot that the rollback discarded. Returns the snapshot that the replay
    /// took, if any, once no computation reads it.
    fn recover_snapshot(&mut self, before: Before) -> Option<CandidateSnapshot> {
        let restored = match before {
            Before::Kept => {
                debug_assert!(self.candidate_snapshot().is_some(), "a failed replay cannot remove a kept snapshot");
                return None;
            }
            Before::NoCandidate => None,
            Before::Discarded(discarded) => Some(discarded),
        };
        if self.candidate_snapshot() == restored.as_ref() {
            return None;
        }

        // A failed fork switch writes nothing to the stable store, so the replay cannot finalize a
        // snapshot: the current one is either final, or a candidate that the replay took.
        let replayed = match &self.kind {
            Kind::Candidate(snapshot) => {
                let snapshot = snapshot.clone();
                self.tasks.cancel();
                self.kind = Kind::Final(snapshot.epoch - 1);
                Some(snapshot)
            }
            Kind::Final(_) => None,
            Kind::Settled(snapshot) => {
                unreachable!("a failed fork switch cannot finalize the candidate snapshot of {}", snapshot.epoch)
            }
        };
        if let Some(restored) = restored {
            restored.trace(Outcome::Restored);
            self.tasks.cancel();
            self.kind = Kind::Candidate(restored);
        }
        replayed
    }

    /// Settled(epoch) -> Final(epoch), deleting the directory, once no computation reads it: its
    /// thread ended, even if no epoch boundary joined it yet. Then publish the stake distributions
    /// again, without the candidate tag: the computation published them with that tag, and the tag
    /// makes the validation of each header walk back to the last block of the snapshot.
    fn release_settled(&mut self, store: &impl Store) {
        let Kind::Settled(snapshot) = &self.kind else { return };
        if self.tasks.is_executing() {
            return;
        }
        let snapshot = snapshot.clone();
        let stake_distribution = match self.tasks.finished() {
            Some(Ok((stake_distribution, _))) => stake_distribution.take(),
            Some(Err(_)) | None => None,
        };
        if let Some(distribution) = stake_distribution {
            self.install_stake_distribution(distribution);
        }

        self.kind = Kind::Final(snapshot.epoch);
        snapshot.remove(store);
        self.publish_stake_distributions();
    }
}

/// LatestSnapshot helper functions
impl LatestSnapshot {
    fn candidate_snapshot(&self) -> Option<&CandidateSnapshot> {
        match &self.kind {
            Kind::Candidate(snapshot) => Some(snapshot),
            Kind::Final(_) | Kind::Settled(_) => None,
        }
    }

    /// Access the snapshot as a store
    fn access<HS: HistoricalStores>(&self, snapshots: &HS) -> Result<HS::Snapshot, StateError> {
        match self.candidate_snapshot() {
            Some(snapshot) => Ok(snapshots.for_candidate_epoch(snapshot.epoch, &snapshot.last_block.hash())?),
            None => Ok(snapshots.for_epoch(self.epoch())?),
        }
    }

    /// The epoch of the computation that reads this snapshot.
    fn current_epoch(&self) -> Epoch {
        self.epoch() + 1
    }

    /// The candidate snapshot that still has a directory on disk, candidate or settled.
    fn on_disk(&self) -> Option<&CandidateSnapshot> {
        match &self.kind {
            Kind::Candidate(snapshot) | Kind::Settled(snapshot) => Some(snapshot),
            Kind::Final(_) => None,
        }
    }
}

/// There are 3 possibilities when we deal with the latest snapshot:
///
///  1. Healthy chain: the snapshot is final because its last block is in the stable DB.
///  2. Sparse chain:
///     2.1 The snapshot last block is volatile so it could be discarded in case of a rollback.
///     2.2 The snapshot is not used anymore but the running computation might still be reading its files.
///
#[derive(Debug, Clone)]
enum Kind {
    /// A snapshot taken from the stable store.
    Final(Epoch),
    /// The last block of its epoch is volatile. A rollback past that block discards it, and the
    /// final snapshot of the epoch before becomes the most recent snapshot again.
    Candidate(CandidateSnapshot),
    /// The last block of its epoch is stable, so the final snapshot of that epoch exists. The
    /// stake distribution and rewards computation can still read the candidate directory until
    /// its thread ends.
    Settled(CandidateSnapshot),
}

/// A snapshot of `epoch` on disk, taken while `last_block`, the last block of that epoch, was
/// still volatile. See [`Store::create_candidate_snapshot`].
///
/// Only this module creates or deletes one: outside it, only its epoch and last block are visible,
/// through [`LatestSnapshot::candidate`].
#[derive(Debug, PartialEq, Clone)]
struct CandidateSnapshot {
    epoch: Epoch,
    last_block: Point,
}

impl CandidateSnapshot {
    /// Delete the directory of this snapshot. A failure only emits a warning since the directory is benign,
    /// and the store deletes all candidate snapshots on open.
    fn remove(self, store: &impl Store) {
        match store.remove_candidate_snapshot(self.epoch, &self.last_block.hash()) {
            Ok(()) => self.trace(Outcome::Removed),
            Err(error) => {
                warn!(
                    ledger::candidate_snapshot::REMOVE_FAILED,
                    epoch = self.epoch,
                    last_block = self.last_block,
                    reason = error.to_string()
                );
            }
        }
    }

    fn trace(&self, outcome: Outcome) {
        info!(
            ledger::candidate_snapshot::UPDATE,
            epoch = self.epoch,
            last_block = self.last_block,
            outcome = outcome.as_str()
        );
    }
}

/// The candidate snapshot as a rollback found it, held until the fork switch ends with
/// [`LatestSnapshot::commit`] or [`LatestSnapshot::recover`].
#[derive(Debug)]
pub(super) struct BeforeRollback {
    candidate: Before,
    /// Whether the computation was done, before the rollback.
    computed: bool,
}

#[derive(Debug)]
enum Before {
    /// No candidate snapshot, or a settled one.
    NoCandidate,
    /// The rollback kept the snapshot.
    Kept,
    /// The rollback removed the last block of the snapshot. Its directory is still on disk.
    Discarded(CandidateSnapshot),
}

#[derive(Debug, Clone, Copy)]
enum Outcome {
    /// The snapshot was taken at the first block after 3k/f.
    Created,
    /// The last block of the snapshot became stable, so the snapshot is final.
    Final,
    /// A rollback removed the last block of the snapshot.
    Discarded,
    /// A failed fork switch put the snapshot back.
    Restored,
    /// The directory of the snapshot was deleted.
    Removed,
}

impl Outcome {
    fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Final => "final",
            Self::Discarded => "discarded",
            Self::Restored => "restored",
            Self::Removed => "removed",
        }
    }
}
