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

//! Atomic pool inspection and bounded delivery of committed membership changes.

use std::{collections::VecDeque, mem::size_of, sync::Arc};

use amaru_kernel::TransactionId;
use amaru_ouroboros_traits::{MempoolSeqNo, TxOrigin};
use parking_lot::Mutex;
use tokio::sync::Notify;

/// Maximum number of live subscriptions to one pool.
pub const MAX_MEMPOOL_SUBSCRIBERS: usize = 64;
/// Maximum number of changes retained by one subscription.
pub const MAX_MEMPOOL_QUEUED_EVENTS: usize = 64;
/// Maximum encoded payload and change metadata retained by one subscription.
pub const MAX_MEMPOOL_QUEUED_BYTES: u64 = 1_048_576;

/// Accepted encoding and attribution, ordered by immutable insertion sequence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MempoolSnapshotEntry {
    pub transaction_id: TransactionId,
    pub sequence: MempoolSeqNo,
    /// The admitting origin. Duplicate offers do not update it.
    pub origin: TxOrigin,
    pub original_bytes: Vec<u8>,
    pub size_bytes: u64,
}

/// Owned membership, order, and counters captured under the pool's lock.
#[derive(Debug, Clone)]
pub struct PoolSnapshot {
    pub generation: u64,
    pub entries: Vec<MempoolSnapshotEntry>,
    pub transaction_count: u64,
    pub total_size_bytes: u64,
    pub capacity_bytes: u64,
}

/// One complete committed membership change. Removals are atomic batches.
#[derive(Debug)]
pub enum PoolChange {
    Inserted { generation: u64, entry: MempoolSnapshotEntry },
    Removed { generation: u64, transaction_ids: Vec<TransactionId> },
}

impl PoolChange {
    pub fn generation(&self) -> u64 {
        match self {
            Self::Inserted { generation, .. } | Self::Removed { generation, .. } => *generation,
        }
    }

    fn size_bytes(&self) -> u64 {
        size_of::<Self>() as u64
            + match self {
                Self::Inserted { entry, .. } => entry.original_bytes.len() as u64,
                Self::Removed { transaction_ids, .. } => (transaction_ids.len() * size_of::<TransactionId>()) as u64,
            }
    }
}

/// Subscription admission or delivery failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PoolObserverError {
    #[error("mempool subscriber limit reached")]
    SubscriberLimit,
    #[error("mempool changes were lost: expected generation {expected_generation}, reached {current_generation}")]
    Gap { expected_generation: u64, current_generation: u64 },
    #[error("mempool stopped")]
    Stopped,
}

#[derive(Debug)]
struct QueueState {
    events: VecDeque<Arc<PoolChange>>,
    bytes: u64,
    next_generation: u64,
    error: Option<PoolObserverError>,
}

#[derive(Debug)]
pub(crate) struct ObserverQueue {
    state: Mutex<QueueState>,
    changed: Notify,
}

impl ObserverQueue {
    fn new(generation: u64) -> Self {
        Self {
            state: Mutex::new(QueueState {
                events: VecDeque::new(),
                bytes: 0,
                next_generation: generation + 1,
                error: None,
            }),
            changed: Notify::new(),
        }
    }

    pub(crate) fn push(&self, event: Arc<PoolChange>) -> bool {
        let mut state = self.state.lock();
        if state.error.is_some() {
            return false;
        }
        let bytes = event.size_bytes();
        if state.events.len() >= MAX_MEMPOOL_QUEUED_EVENTS
            || state.bytes.saturating_add(bytes) > MAX_MEMPOOL_QUEUED_BYTES
        {
            state.error = Some(PoolObserverError::Gap {
                expected_generation: state.next_generation,
                current_generation: event.generation(),
            });
            state.events.clear();
            state.bytes = 0;
            self.changed.notify_one();
            return false;
        }
        state.bytes += bytes;
        state.events.push_back(event);
        self.changed.notify_one();
        true
    }

    pub(crate) fn close(&self) {
        let mut state = self.state.lock();
        state.events.clear();
        state.bytes = 0;
        state.error = Some(PoolObserverError::Stopped);
        self.changed.notify_one();
    }
}

/// A bounded receiver. A gap permanently invalidates it; subscribe again to recover.
///
/// This owns only queued changes, not the pool. Each queue holds at most
/// [`MAX_MEMPOOL_QUEUED_EVENTS`] changes and [`MAX_MEMPOOL_QUEUED_BYTES`] encoded
/// payload and metadata bytes. Queues retain no parsed transactions.
pub struct PoolReceiver {
    pub(crate) queue: Arc<ObserverQueue>,
}

impl PoolReceiver {
    pub(crate) fn new(generation: u64) -> Self {
        Self { queue: Arc::new(ObserverQueue::new(generation)) }
    }

    /// Receive the next complete generation, or an explicit terminal failure.
    pub async fn recv(&mut self) -> Result<Arc<PoolChange>, PoolObserverError> {
        loop {
            let changed = self.queue.changed.notified();
            {
                let mut state = self.queue.state.lock();
                if let Some(error) = state.error {
                    return Err(error);
                }
                if let Some(event) = state.events.pop_front() {
                    state.bytes -= event.size_bytes();
                    state.next_generation = event.generation() + 1;
                    return Ok(event);
                }
            }
            changed.await;
        }
    }
}
