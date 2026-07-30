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

//! Shared performance resource: peer quality / availability and header lifecycle timings.
//!
//! [`PeerPerformance`] and [`HeaderPerformance`] are owned by a dedicated worker thread that
//! runs a Tokio `current_thread` runtime and pulls operations from an unbounded channel.
//! Stages record **events** via external effects constructed on [`Performance`], e.g.
//! `eff.external(Performance::record_header_announcement(...)).await`. Query effects await a
//! `tokio::sync::oneshot` reply (no blocking of multi-thread Tokio workers).
//!
//! Unit tests for peer/header logic should construct those types directly without spawning this
//! handle; the resource thread is only needed when exercising the effect path.
//!
//! Channel depth is monitored: WARN (rate-limited) when the queue exceeds normally expected
//! depth, ERROR + panic when it grows beyond reasonable bounds.
//!
//! OTel events and metrics are emitted inside the worker when terminal outcomes are recorded.

mod effects;
mod header;
mod peer;

use std::{
    fmt,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use amaru_kernel::Peer;
use amaru_metrics::Meter;
use amaru_pure_stage::Instant;
pub use effects::*;
pub use header::{ForkSwitchOutcome, HeaderLifecycleOutcome, HeaderPerformance};
use parking_lot::Mutex;
pub use peer::{BlockClaim, ClaimKind, FetchPeerSet, PeerPerformance, PeerScores, PeerSnapshot, SelectPeersParams};
use tokio::{
    sync::{
        mpsc::{UnboundedSender, unbounded_channel},
        oneshot,
    },
    time::Instant as TokioInstant,
};
use tracing::{error, warn};

/// Resource type installed in pure-stage `Resources`.
pub type ResourcePerformance = Arc<Performance>;

/// Depth at which a WARN is logged for the performance op queue.
pub const QUEUE_WARN_THRESHOLD: usize = 1000;
/// Depth at which an ERROR is logged for the performance op queue.
pub const QUEUE_ERROR_THRESHOLD: usize = 100_000;
/// Minimum interval between successive queue-depth WARN logs.
const QUEUE_WARN_MIN_INTERVAL: Duration = Duration::from_secs(1);

/// Joins the worker when the last [`Performance`] handle is dropped (after senders close the channel).
///
/// Dropping the last [`Performance`] closes the unbounded op channel and then blocks in
/// [`JoinHandle::join`] until the worker has drained remaining ops and exited. Prefer dropping
/// from node teardown rather than a multi-thread Tokio worker task, so that drain/join does not
/// stall a runtime thread under a deep queue.
struct WorkerGuard {
    join: Mutex<Option<JoinHandle<()>>>,
}

impl Drop for WorkerGuard {
    fn drop(&mut self) {
        if let Some(handle) = self.join.lock().take()
            && let Err(_panic) = handle.join()
        {
            error!(target: "amaru_consensus::performance", "performance worker thread panicked");
        }
    }
}

/// Handle to the performance subsystem (Send + Sync). State lives on a worker thread.
///
/// Field order matters for cleanup: `tx` is dropped before the worker join guard, so the last
/// sender closes the op channel and the worker can exit before the join runs.
///
/// Dropping the last clone joins the worker and waits while it drains any remaining ops.
pub struct Performance {
    tx: UnboundedSender<PerformanceOp>,
    /// Approximate number of ops queued or being processed (incremented before send).
    pending: Arc<AtomicUsize>,
    /// Monotonic time of the last queue-depth WARN (for 1/s rate limiting).
    ///
    /// Uses [`tokio::time::Instant`] rather than [`std::time::Instant`]: on some platforms a
    /// buggy monotonic clock can go backwards and make `std` panics in `duration_since`, while
    /// Tokio's Instant is safe to read outside a runtime (see unit test) and saturates.
    last_queue_warn: Arc<Mutex<Option<TokioInstant>>>,
    worker: Arc<WorkerGuard>,
}

impl Clone for Performance {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
            pending: Arc::clone(&self.pending),
            last_queue_warn: Arc::clone(&self.last_queue_warn),
            worker: Arc::clone(&self.worker),
        }
    }
}

impl fmt::Debug for Performance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Performance")
            .field("queue_depth", &self.pending.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

/// All operations accepted by the performance worker, wrapping the corresponding effect payloads.
pub(crate) enum PerformanceOp {
    RecordIntersection { effect: RecordIntersectionEffect },
    RecordHeaderAnnouncement { effect: RecordHeaderAnnouncementEffect },
    RecordBlocksRequested { effect: RecordBlocksRequestedEffect },
    RecordBlockDelivery { effect: RecordBlockDeliveryEffect },
    RecordFetchFailure { effect: RecordFetchFailureEffect },
    RecordKeepaliveRtt { effect: RecordKeepaliveRttEffect },
    ClearPeerAvailability { effect: ClearPeerAvailabilityEffect },
    ForgetPeer { effect: ForgetPeerEffect },
    PruneBelow { effect: PruneBelowEffect },
    SelectPeersForFetch { effect: SelectPeersForFetchEffect, reply: oneshot::Sender<FetchPeerSet> },
    PeerCoversFragment { effect: PeerCoversFragmentEffect, reply: oneshot::Sender<bool> },
    DirectClaimants { effect: DirectClaimantsEffect, reply: oneshot::Sender<Vec<(Peer, Instant, ClaimKind)>> },
    FirstAnnouncedAt { effect: FirstAnnouncedAtEffect, reply: oneshot::Sender<Option<(Peer, Instant)>> },
    RankPeersForChurn { effect: RankPeersForChurnEffect, reply: oneshot::Sender<Vec<(Peer, PeerScores)>> },
    Scores { effect: ScoresEffect, reply: oneshot::Sender<PeerScores> },
    Snapshot { effect: SnapshotEffect, reply: oneshot::Sender<Option<PeerSnapshot>> },
    RecordRollback { effect: RecordRollbackEffect },
    RecordHeaderRejected { effect: RecordHeaderRejectedEffect, meter: Option<Arc<Meter>> },
    RecordHeaderAbandoned { effect: RecordHeaderAbandonedEffect, meter: Option<Arc<Meter>> },
    RecordForkStarted { effect: RecordForkStartedEffect, meter: Option<Arc<Meter>> },
    RecordBlockValid { effect: RecordBlockValidEffect, meter: Option<Arc<Meter>> },
    RecordBlockPruned { effect: RecordBlockPrunedEffect, meter: Option<Arc<Meter>> },
}

impl Performance {
    /// Start the performance worker thread and return a handle to enqueue operations.
    ///
    /// Dropping the last clone of this handle closes the op channel and joins the worker thread
    /// (after it drains remaining ops). Prefer that drop on a non-hot path (node teardown), not
    /// from a multi-thread Tokio worker task.
    pub fn new() -> Self {
        let (tx, mut rx) = unbounded_channel::<PerformanceOp>();
        let pending = Arc::new(AtomicUsize::new(0));
        let pending_worker = Arc::clone(&pending);

        #[expect(clippy::expect_used)]
        let join = thread::Builder::new()
            .name("performance".into())
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("performance worker runtime");
                rt.block_on(async move {
                    let mut peers = PeerPerformance::new();
                    let mut headers = HeaderPerformance::new();
                    while let Some(op) = rx.recv().await {
                        pending_worker.fetch_sub(1, Ordering::Relaxed);
                        dispatch(&mut peers, &mut headers, op);
                    }
                });
            })
            .expect("failed to spawn performance worker thread");

        Self {
            tx,
            pending,
            last_queue_warn: Arc::new(Mutex::new(None)),
            worker: Arc::new(WorkerGuard { join: Mutex::new(Some(join)) }),
        }
    }

    /// Enqueue an operation. Updates the pending counter and logs WARN/ERROR thresholds.
    pub(crate) fn submit(&self, op: PerformanceOp) {
        let depth = self.pending.fetch_add(1, Ordering::Relaxed) + 1;
        if depth > QUEUE_WARN_THRESHOLD && self.should_log_queue_warn() {
            warn!(
                target: "amaru_consensus::performance",
                queue_depth = depth,
                "observability system lagging behind"
            );
        }
        #[expect(clippy::panic)]
        if depth == QUEUE_ERROR_THRESHOLD + 1 {
            error!(
                target: "amaru_consensus::performance",
                queue_depth = depth,
                "performance op queue exceeded {QUEUE_ERROR_THRESHOLD}"
            );
            // NOTE: Amaru fails loudly and early when design assumptions are dynamically
            // violated. The performance worker is expected to keep pace with consensus stages;
            // an unbounded queue past this depth means that assumption no longer holds, so we
            // panic rather than silently drop telemetry or grow without bound.
            panic!("performance op queue exceeded {QUEUE_ERROR_THRESHOLD}");
        }
        // If the worker has died, drop the op; the pending counter will be slightly wrong.
        if self.tx.send(op).is_err() {
            self.pending.fetch_sub(1, Ordering::Relaxed);
        }
    }

    /// Approximate number of ops queued or in flight.
    pub fn queue_depth(&self) -> usize {
        self.pending.load(Ordering::Relaxed)
    }

    /// Returns true at most once per [`QUEUE_WARN_MIN_INTERVAL`] across all producers.
    /// Uses a monotonic clock so wall-clock adjustments do not suppress or burst logs.
    fn should_log_queue_warn(&self) -> bool {
        let now = TokioInstant::now();
        let mut last = self.last_queue_warn.lock();
        match *last {
            Some(prev) if now.duration_since(prev) < QUEUE_WARN_MIN_INTERVAL => false,
            _ => {
                *last = Some(now);
                true
            }
        }
    }
}

impl Default for Performance {
    fn default() -> Self {
        Self::new()
    }
}

fn dispatch(peers: &mut PeerPerformance, headers: &mut HeaderPerformance, op: PerformanceOp) {
    match op {
        PerformanceOp::RecordIntersection { effect } => {
            peers.apply_intersection(effect.peer, effect.current, effect.parent, effect.at);
        }
        PerformanceOp::RecordHeaderAnnouncement { effect } => {
            peers.apply_header_announcement(effect.peer.clone(), effect.header, effect.parent, effect.at);
            headers.apply_header_received(effect.peer, effect.header, effect.at, effect.slot_start_to_header_micros);
        }
        PerformanceOp::RecordBlocksRequested { effect } => {
            headers.apply_blocks_requested(&effect.hashes, effect.requested_at);
        }
        PerformanceOp::RecordBlockDelivery { effect } => {
            peers.apply_block_delivery(
                effect.peer,
                effect.hash,
                effect.height,
                effect.parent,
                effect.at,
                effect.response,
                effect.bytes,
            );
            headers.apply_block_downloaded(&effect.hash, effect.at);
        }
        PerformanceOp::RecordFetchFailure { effect } => {
            peers.apply_fetch_failure(&effect.peers, effect.at);
        }
        PerformanceOp::RecordKeepaliveRtt { effect } => {
            peers.apply_keepalive_rtt(effect.peer, effect.rtt, effect.at);
        }
        PerformanceOp::ClearPeerAvailability { effect } => {
            peers.apply_clear_peer_availability(&effect.peer);
        }
        PerformanceOp::ForgetPeer { effect } => {
            peers.apply_forget_peer(&effect.peer);
        }
        PerformanceOp::PruneBelow { effect } => {
            peers.apply_prune_below(effect.min_height);
        }
        PerformanceOp::SelectPeersForFetch { effect, reply } => {
            let result = peers.apply_select_peers_for_fetch(effect.params);
            let _ = reply.send(result);
        }
        PerformanceOp::PeerCoversFragment { effect, reply } => {
            let result = peers.apply_peer_covers_fragment(&effect.peer, &effect.need);
            let _ = reply.send(result);
        }
        PerformanceOp::DirectClaimants { effect, reply } => {
            let result = peers.apply_direct_claimants(&effect.hash);
            let _ = reply.send(result);
        }
        PerformanceOp::FirstAnnouncedAt { effect, reply } => {
            let result = peers.apply_first_announced_at(&effect.hash);
            let _ = reply.send(result);
        }
        PerformanceOp::RankPeersForChurn { effect, reply } => {
            let result = peers.apply_rank_peers_for_churn(&effect.candidates, effect.now);
            let _ = reply.send(result);
        }
        PerformanceOp::Scores { effect, reply } => {
            let result = peers.apply_scores(&effect.peer);
            let _ = reply.send(result);
        }
        PerformanceOp::Snapshot { effect, reply } => {
            let result = peers.apply_snapshot(&effect.peer);
            let _ = reply.send(result);
        }
        PerformanceOp::RecordRollback { effect } => {
            peers.apply_rollback(effect.peer, effect.point, effect.parent, effect.at);
        }
        PerformanceOp::RecordHeaderRejected { effect, meter } => {
            headers.apply_header_rejected(effect.outcome, meter.as_deref());
        }
        PerformanceOp::RecordHeaderAbandoned { effect, meter } => {
            headers.apply_header_abandoned(&effect.hash, effect.now, meter.as_deref());
        }
        PerformanceOp::RecordForkStarted { effect, meter } => {
            headers.apply_fork_started(effect.tip, effect.started_at, meter.as_deref());
        }
        PerformanceOp::RecordBlockValid { effect, meter } => {
            headers.apply_block_valid(&effect.hash, effect.now, effect.syncing, meter.as_deref());
        }
        PerformanceOp::RecordBlockPruned { effect, meter } => {
            headers.apply_block_pruned(&effect.hash, effect.invalid, effect.now, effect.syncing, meter.as_deref());
        }
    }
}

#[cfg(test)]
mod tests;
