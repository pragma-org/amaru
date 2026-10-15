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
pub struct PoolSnapshot {
    /// Revision within this pool; starts at zero on restart. Revalidation without removals does not advance it.
    pub generation: u64,
    pub entries: Vec<MempoolSnapshotEntry>,
    pub transaction_count: u64,
    pub total_size_bytes: u64,
    pub capacity_bytes: u64,
}

/// One complete committed membership change. Removals are atomic batches.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PoolChange {
    Inserted {
        generation: u64,
        entry: MempoolSnapshotEntry,
    },
    /// Committed removal, including transactions that failed automatic revalidation.
    /// Removal alone does not establish chain inclusion or confirmation.
    Removed {
        generation: u64,
        transaction_ids: Vec<TransactionId>,
    },
}

impl PoolChange {
    pub fn generation(&self) -> u64 {
        match self {
            Self::Inserted { generation, .. } | Self::Removed { generation, .. } => *generation,
        }
    }
}
