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

//! Atomic pool snapshots and committed membership changes.

use std::{sync::Arc, time::SystemTime};

use amaru_kernel::TransactionId;
use amaru_ouroboros_traits::{MempoolSeqNo, TxOrigin};
use tokio::sync::broadcast;

/// Shared event history. Slow subscribers receive an explicit lag error.
pub const MAX_MEMPOOL_QUEUED_EVENTS: usize = 64;
pub type PoolReceiver = broadcast::Receiver<Arc<PoolChange>>;

/// Identity of a membership revision and how to advance it after a committed change.
pub trait Generation: Clone {
    fn next(&self) -> Self;
}

impl Generation for u64 {
    fn next(&self) -> Self {
        self + 1
    }
}

/// Accepted encoding and attribution, ordered by immutable insertion sequence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MempoolSnapshotEntry {
    pub transaction_id: TransactionId,
    pub sequence: MempoolSeqNo,
    /// The admitting origin. Duplicate offers do not update it.
    pub origin: TxOrigin,
    /// Wall-clock insertion time; use the sequence for ordering because wall-clock time can jump.
    pub admitted_at: SystemTime,
    pub original_bytes: Vec<u8>,
    pub size_bytes: u64,
}

/// Owned membership, order, and counters captured under the pool's lock.
///
/// This capture does not wait for ledger revalidation or establish validity against
/// a particular ledger point. Entries awaiting revalidation remain stored until removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolSnapshot<G: Generation = u64> {
    /// Membership revision; a revalidation pass without removals does not advance it.
    pub generation: G,
    pub entries: Vec<MempoolSnapshotEntry>,
    pub transaction_count: u64,
    pub total_size_bytes: u64,
    pub capacity_bytes: u64,
}

impl<G: Generation> PoolSnapshot<G> {
    /// Transform the generation while preserving captured membership and counters.
    pub fn map_generation<T: Generation>(self, map: impl FnOnce(G) -> T) -> PoolSnapshot<T> {
        PoolSnapshot {
            generation: map(self.generation),
            entries: self.entries,
            transaction_count: self.transaction_count,
            total_size_bytes: self.total_size_bytes,
            capacity_bytes: self.capacity_bytes,
        }
    }
}

/// One complete committed membership change. Removals are atomic batches.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PoolChange<G: Generation = u64> {
    Inserted {
        generation: G,
        entry: MempoolSnapshotEntry,
    },
    /// Committed removal, including transactions that failed automatic revalidation.
    /// Removal alone does not establish chain inclusion or confirmation.
    Removed {
        generation: G,
        transaction_ids: Vec<TransactionId>,
    },
}

impl<G: Generation> PoolChange<G> {
    pub fn generation(&self) -> G {
        match self {
            Self::Inserted { generation, .. } | Self::Removed { generation, .. } => generation.clone(),
        }
    }

    /// Transform the generation while preserving the complete committed change.
    pub fn map_generation<T: Generation>(self, map: impl FnOnce(G) -> T) -> PoolChange<T> {
        match self {
            Self::Inserted { generation, entry } => PoolChange::Inserted { generation: map(generation), entry },
            Self::Removed { generation, transaction_ids } => {
                PoolChange::Removed { generation: map(generation), transaction_ids }
            }
        }
    }
}
