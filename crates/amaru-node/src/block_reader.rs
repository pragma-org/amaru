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

//! Bounded immutable reads for embedding hosts with independently owned worker tasks.

use std::sync::{Arc, Weak};

use amaru_kernel::{NetworkName, NetworkPoint};
use amaru_ouroboros::{BlockBatch, BlockRange, BlockReadError, BlockReadLimits, ChainStore, ResolvedBlockRange};
use parking_lot::RwLock;
use tokio_util::sync::CancellationToken;

/// Cooperative per-scan cancellation. Node shutdown also cancels every scan.
pub type BlockReadCancellation = CancellationToken;

/// A cloneable, non-owning handle that can move into `spawn_blocking`.
/// It holds neither the node nor its store alive. All methods perform blocking storage reads.
#[derive(Clone)]
pub struct BlockReader {
    state: Weak<ReaderState>,
    network: NetworkName,
}

/// A captured immutable target on a particular network.
///
/// This metadata is available even for an empty range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockBoundary {
    pub network: NetworkName,
    pub through: NetworkPoint,
}

/// A resolved scan retaining only metadata and weak access to the node's store.
///
/// Each [`Self::read_batch`] uses and releases a fresh snapshot. Reopening with the same
/// explicit `through` preserves the target as the immutable anchor advances. If an endpoint
/// disappears or history is pruned between batches, reading fails; it never skips forward or
/// reports completion. Persist index changes and `last_point` in one application transaction,
/// then resume from that committed point. Persist `boundary` only after a complete batch.
///
/// There is no automatic cursor advancement: a failed application transaction can safely
/// retry the same `after`. A snapshot is never held while the application commits its changes.
#[derive(Clone)]
pub struct BlockScan {
    reader: BlockReader,
    range: ResolvedBlockRange,
    cancellation: BlockReadCancellation,
}

struct ReaderState {
    store: RwLock<Option<Arc<dyn ChainStore>>>,
    cancellation: CancellationToken,
    network: NetworkName,
}

pub(crate) struct BlockReaderOwner(Arc<ReaderState>);

impl BlockReaderOwner {
    pub(crate) fn new(store: Arc<dyn ChainStore>, network: NetworkName) -> Self {
        Self(Arc::new(ReaderState { store: RwLock::new(Some(store)), cancellation: CancellationToken::new(), network }))
    }

    pub(crate) fn reader(&self) -> BlockReader {
        BlockReader { state: Arc::downgrade(&self.0), network: self.0.network }
    }

    pub(crate) fn cancellation(&self) -> CancellationToken {
        self.0.cancellation.clone()
    }

    /// Called on the blocking pool: cancellation wakes cooperative reads before draining snapshots.
    pub(crate) fn close(self) {
        self.0.cancellation.cancel();
        self.0.store.write().take();
    }
}

impl Drop for BlockReaderOwner {
    fn drop(&mut self) {
        self.0.cancellation.cancel();
    }
}

impl BlockReader {
    /// Capture and validate an immutable target without walking the remaining ancestry.
    /// Cancellation and the storage-step budget apply during opening as well as batching.
    /// Same-slot blocks are checked through parent links with bounded work.
    pub fn open(
        &self,
        range: BlockRange,
        limits: BlockReadLimits,
        cancellation: BlockReadCancellation,
    ) -> Result<BlockScan, BlockReadError> {
        let range =
            self.with_store(&cancellation, |store, cancelled| store.open_block_range(range, limits, cancelled))?;
        Ok(BlockScan { reader: self.clone(), range, cancellation })
    }

    fn with_store<T>(
        &self,
        cancellation: &BlockReadCancellation,
        read: impl FnOnce(&dyn ChainStore, &dyn Fn() -> bool) -> Result<T, BlockReadError>,
    ) -> Result<T, BlockReadError> {
        let state = self.state.upgrade().ok_or(BlockReadError::Closed)?;
        let cancelled = || state.cancellation.is_cancelled() || cancellation.is_cancelled();
        if cancelled() {
            return Err(BlockReadError::Cancelled);
        }
        let guard = state.store.read();
        let store = guard.as_deref().ok_or(BlockReadError::Closed)?;
        read(store, &cancelled)
    }
}

impl BlockScan {
    /// Network identity and fixed inclusive target, including for empty ranges.
    pub fn boundary(&self) -> BlockBoundary {
        BlockBoundary { network: self.reader.network, through: self.range.through }
    }

    /// The validated initial checkpoint and captured target.
    pub fn range(&self) -> ResolvedBlockRange {
        self.range
    }

    /// Read after the application's committed checkpoint, at most the configured blocks and bytes.
    /// A storage-step limit or cancellation returns an error without exposing partial results.
    /// One storage call (including decoding one block) cannot be interrupted. The byte limit bounds
    /// returned bodies; reading may temporarily allocate one additional body to check its size.
    pub fn read_batch(&self, after: NetworkPoint, limits: BlockReadLimits) -> Result<BlockBatch, BlockReadError> {
        self.reader.with_store(&self.cancellation, |store, cancelled| {
            store.read_block_batch(
                BlockRange { after: Some(after), through: Some(self.range.through) },
                limits,
                cancelled,
            )
        })
    }
}
