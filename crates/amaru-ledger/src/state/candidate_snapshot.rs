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

use std::mem;

use amaru_kernel::{Epoch, Point};
use amaru_observability::{info, warn};

use crate::store::Store;

/// A snapshot of `epoch` on disk, taken while `last_block`, the last block of that epoch, was
/// still volatile. See [`Store::create_candidate_snapshot`].
///
/// Only this module creates or deletes one: outside it, a snapshot is only ever borrowed from
/// [`CandidateSnapshotState`].
#[derive(Debug, PartialEq)]
pub(super) struct CandidateSnapshot {
    epoch: Epoch,
    last_block: Point,
}

impl CandidateSnapshot {
    pub fn epoch(&self) -> Epoch {
        self.epoch
    }

    pub fn last_block(&self) -> Point {
        self.last_block
    }

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

/// The candidate snapshot of the closing epoch, from its creation to the removal of its
/// directory.
///
/// ```text
///            create            settle              release_settled
///   Absent ─────────▶ Candidate ─────────▶ Settled ─────────────────▶ Absent
///      ▲                  │
///      └──────────────────┘
///        roll_back (past the last block), recover
/// ```
#[derive(Debug, Default)]
pub(super) struct CandidateSnapshotState(Lifecycle);

#[derive(Debug, Default)]
enum Lifecycle {
    #[default]
    Absent,
    /// The last block of its epoch is volatile. A rollback past that block discards it.
    Candidate(CandidateSnapshot),
    /// The last block of its epoch is stable, so the final snapshot of that epoch exists. The
    /// stake distribution and rewards computation can still read it until the next epoch
    /// boundary joins that computation.
    Settled(CandidateSnapshot),
}

/// What a rollback did to the candidate snapshot.
#[derive(Debug, PartialEq)]
pub(super) enum RolledBack {
    /// The rollback stayed after the epoch of the snapshot, or there is no candidate snapshot.
    Untouched,
    /// The rollback went back into the epoch of the snapshot. The snapshot is discarded when the
    /// rollback also removed its last block. Any computation that started in the next epoch is
    /// now stale.
    IntoEpoch,
}

impl CandidateSnapshotState {
    pub fn is_absent(&self) -> bool {
        matches!(self.0, Lifecycle::Absent)
    }

    /// The snapshot, while the last block of its epoch is volatile.
    pub fn candidate(&self) -> Option<&CandidateSnapshot> {
        match &self.0 {
            Lifecycle::Candidate(snapshot) => Some(snapshot),
            Lifecycle::Absent | Lifecycle::Settled(_) => None,
        }
    }

    /// The snapshot that still has a directory on disk, candidate or settled.
    pub fn on_disk(&self) -> Option<&CandidateSnapshot> {
        match &self.0 {
            Lifecycle::Candidate(snapshot) | Lifecycle::Settled(snapshot) => Some(snapshot),
            Lifecycle::Absent => None,
        }
    }

    pub fn epoch(&self) -> Option<Epoch> {
        self.on_disk().map(CandidateSnapshot::epoch)
    }

    /// Absent -> Candidate, once the snapshot of `epoch` ending with `last_block` is written to disk.
    pub fn create(&mut self, epoch: Epoch, last_block: Point) {
        debug_assert!(self.is_absent(), "a candidate snapshot already exists: {self:?}");
        let snapshot = CandidateSnapshot { epoch, last_block };
        snapshot.trace(Outcome::Created);
        self.0 = Lifecycle::Candidate(snapshot);
    }

    /// Candidate -> Settled, once the final snapshot of its epoch exists.
    pub fn settle(&mut self) {
        self.0 = match mem::take(&mut self.0) {
            Lifecycle::Candidate(snapshot) => {
                snapshot.trace(Outcome::Final);
                Lifecycle::Settled(snapshot)
            }
            other @ (Lifecycle::Absent | Lifecycle::Settled(_)) => other,
        };
    }

    /// Settled -> Absent, deleting the directory, once no computation reads it. Returns `true`
    /// when a snapshot was released.
    pub fn release_settled(&mut self, store: &impl Store) -> bool {
        match mem::take(&mut self.0) {
            Lifecycle::Settled(snapshot) => {
                snapshot.remove(store);
                true
            }
            other @ (Lifecycle::Absent | Lifecycle::Candidate(_)) => {
                self.0 = other;
                false
            }
        }
    }

    /// Candidate -> Absent. The caller owns the directory.
    fn take_candidate(&mut self) -> Option<CandidateSnapshot> {
        match mem::take(&mut self.0) {
            Lifecycle::Candidate(snapshot) => Some(snapshot),
            other @ (Lifecycle::Absent | Lifecycle::Settled(_)) => {
                self.0 = other;
                None
            }
        }
    }

    /// Candidate -> Absent when a rollback to `volatile_epoch` removed the last block of the
    /// snapshot. The returned [`BeforeRollback`] owns the discarded snapshot, whose directory stays
    /// on disk until the fork switch ends with [`Self::commit`] or [`Self::recover`].
    pub fn roll_back(
        &mut self,
        volatile_epoch: Epoch,
        has_point: impl FnOnce(&Point) -> bool,
    ) -> (RolledBack, BeforeRollback) {
        let Some(snapshot) = self.candidate() else {
            return (RolledBack::Untouched, BeforeRollback(Before::NoCandidate));
        };
        if volatile_epoch > snapshot.epoch {
            return (RolledBack::Untouched, BeforeRollback(Before::Kept));
        }
        if has_point(&snapshot.last_block) {
            return (RolledBack::IntoEpoch, BeforeRollback(Before::Kept));
        }
        let Some(discarded) = self.take_candidate() else { unreachable!("the candidate snapshot was just borrowed") };
        discarded.trace(Outcome::Discarded);
        (RolledBack::IntoEpoch, BeforeRollback(Before::Discarded(discarded)))
    }

    /// End a fork switch that keeps the new chain: delete the directory of the snapshot that the
    /// rollback discarded, unless the replay took the same snapshot again.
    pub fn commit(&self, before: BeforeRollback, store: &impl Store) {
        if let Before::Discarded(discarded) = before.0
            && self.on_disk() != Some(&discarded)
        {
            discarded.remove(store);
        }
    }

    /// End a failed fork switch, once the volatile state of the old chain is back: delete any
    /// snapshot the replay took, and put back the one the rollback discarded. `stop_readers` runs
    /// before any directory is deleted. Returns `true` when the snapshot changed.
    pub fn recover(&mut self, before: BeforeRollback, store: &impl Store, stop_readers: impl FnOnce()) -> bool {
        let restored = match before.0 {
            Before::Kept => {
                debug_assert!(self.candidate().is_some(), "a failed replay cannot remove a kept snapshot");
                return false;
            }
            Before::NoCandidate => None,
            Before::Discarded(discarded) => Some(discarded),
        };
        if self.candidate() == restored.as_ref() {
            return false;
        }
        stop_readers();
        if let Some(replayed) = self.take_candidate() {
            replayed.remove(store);
        }
        if let Some(discarded) = restored {
            discarded.trace(Outcome::Restored);
            self.0 = Lifecycle::Candidate(discarded);
        }
        true
    }
}

/// The candidate snapshot as a rollback found it, held until the fork switch ends with
/// [`CandidateSnapshotState::commit`] or [`CandidateSnapshotState::recover`].
#[derive(Debug)]
pub(super) struct BeforeRollback(Before);

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
