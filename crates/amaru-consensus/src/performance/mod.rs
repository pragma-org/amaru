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
//! Channel depth is monitored: WARN when the queue exceeds 100 pending ops, ERROR above 1000.
//!
//! OTel events and metrics are emitted inside the worker when terminal outcomes are recorded.

mod effects;
mod header;
mod peer;

use std::{
    fmt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    thread::{self, JoinHandle},
};

use amaru_kernel::Peer;
use amaru_metrics::Meter;
use amaru_pure_stage::Instant;
pub use effects::*;
pub use header::{ForkSwitchOutcome, HeaderLifecycleOutcome, HeaderPerformance};
pub use peer::{BlockClaim, ClaimKind, FetchPeerSet, PeerPerformance, PeerScores, PeerSnapshot, SelectPeersParams};
use tokio::sync::{
    mpsc::{UnboundedSender, unbounded_channel},
    oneshot,
};
use tracing::{error, warn};

/// Resource type installed in pure-stage `Resources`.
pub type ResourcePerformance = Arc<Performance>;

/// Depth at which a WARN is logged for the performance op queue.
pub const QUEUE_WARN_THRESHOLD: usize = 100;
/// Depth at which an ERROR is logged for the performance op queue.
pub const QUEUE_ERROR_THRESHOLD: usize = 1000;

/// Joins the worker when the last [`Performance`] handle is dropped (after senders close the channel).
struct WorkerGuard {
    join: Mutex<Option<JoinHandle<()>>>,
}

impl Drop for WorkerGuard {
    fn drop(&mut self) {
        // Poisoned mutex still carries the JoinHandle; recover so we can join.
        let mut guard = self.join.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(handle) = guard.take()
            && let Err(_panic) = handle.join()
        {
            error!(target: "amaru_consensus::performance", "performance worker thread panicked");
        }
    }
}

/// Handle to the performance subsystem (Send + Sync). State lives on a worker thread.
///
/// Field order matters for cleanup: `tx` is dropped before `worker`, so the last sender
/// closes the op channel and the worker can exit before `WorkerGuard` joins the thread.
pub struct Performance {
    tx: UnboundedSender<PerformanceOp>,
    /// Approximate number of ops queued or being processed (incremented before send).
    pending: Arc<AtomicUsize>,
    worker: Arc<WorkerGuard>,
}

impl Clone for Performance {
    fn clone(&self) -> Self {
        Self { tx: self.tx.clone(), pending: Arc::clone(&self.pending), worker: Arc::clone(&self.worker) }
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
pub enum PerformanceOp {
    RecordIntersection {
        effect: RecordIntersectionEffect,
        done: Option<oneshot::Sender<()>>,
    },
    RecordHeaderAnnouncement {
        effect: RecordHeaderAnnouncementEffect,
        done: Option<oneshot::Sender<()>>,
    },
    RecordBlocksRequested {
        effect: RecordBlocksRequestedEffect,
        done: Option<oneshot::Sender<()>>,
    },
    RecordBlockDelivery {
        effect: RecordBlockDeliveryEffect,
        done: Option<oneshot::Sender<()>>,
    },
    RecordFetchFailure {
        effect: RecordFetchFailureEffect,
        done: Option<oneshot::Sender<()>>,
    },
    RecordKeepaliveRtt {
        effect: RecordKeepaliveRttEffect,
        done: Option<oneshot::Sender<()>>,
    },
    ClearPeerAvailability {
        effect: ClearPeerAvailabilityEffect,
        done: Option<oneshot::Sender<()>>,
    },
    ForgetPeer {
        effect: ForgetPeerEffect,
        done: Option<oneshot::Sender<()>>,
    },
    PruneBelow {
        effect: PruneBelowEffect,
        done: Option<oneshot::Sender<()>>,
    },
    SelectPeersForFetch {
        effect: SelectPeersForFetchEffect,
        reply: oneshot::Sender<FetchPeerSet>,
    },
    PeerCoversFragment {
        effect: PeerCoversFragmentEffect,
        reply: oneshot::Sender<bool>,
    },
    DirectClaimants {
        effect: DirectClaimantsEffect,
        reply: oneshot::Sender<Vec<(Peer, Instant, ClaimKind)>>,
    },
    FirstAnnouncedAt {
        effect: FirstAnnouncedAtEffect,
        reply: oneshot::Sender<Option<(Peer, Instant)>>,
    },
    RankPeersForChurn {
        effect: RankPeersForChurnEffect,
        reply: oneshot::Sender<Vec<(Peer, PeerScores)>>,
    },
    Scores {
        effect: ScoresEffect,
        reply: oneshot::Sender<PeerScores>,
    },
    Snapshot {
        effect: SnapshotEffect,
        reply: oneshot::Sender<Option<PeerSnapshot>>,
    },
    RecordRollback {
        effect: RecordRollbackEffect,
        done: Option<oneshot::Sender<()>>,
    },
    RecordHeaderRejected {
        effect: RecordHeaderRejectedEffect,
        meter: Option<Arc<Meter>>,
        done: Option<oneshot::Sender<()>>,
    },
    RecordHeaderAbandoned {
        effect: RecordHeaderAbandonedEffect,
        meter: Option<Arc<Meter>>,
        done: Option<oneshot::Sender<()>>,
    },
    RecordForkStarted {
        effect: RecordForkStartedEffect,
        meter: Option<Arc<Meter>>,
        done: Option<oneshot::Sender<()>>,
    },
    RecordBlockValid {
        effect: RecordBlockValidEffect,
        meter: Option<Arc<Meter>>,
        done: Option<oneshot::Sender<()>>,
    },
    RecordBlockPruned {
        effect: RecordBlockPrunedEffect,
        meter: Option<Arc<Meter>>,
        done: Option<oneshot::Sender<()>>,
    },
}

impl Performance {
    /// Start the performance worker thread and return a handle to enqueue operations.
    ///
    /// Dropping the last clone of this handle closes the op channel and joins the worker thread.
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

        Self { tx, pending, worker: Arc::new(WorkerGuard { join: Mutex::new(Some(join)) }) }
    }

    /// Enqueue an operation. Updates the pending counter and logs WARN/ERROR thresholds.
    pub(crate) fn submit(&self, op: PerformanceOp) {
        let depth = self.pending.fetch_add(1, Ordering::Relaxed) + 1;
        if depth == QUEUE_WARN_THRESHOLD + 1 {
            warn!(
                target: "amaru_consensus::performance",
                queue_depth = depth,
                "performance op queue exceeded {QUEUE_WARN_THRESHOLD}"
            );
        }
        #[expect(clippy::panic)]
        if depth == QUEUE_ERROR_THRESHOLD + 1 {
            error!(
                target: "amaru_consensus::performance",
                queue_depth = depth,
                "performance op queue exceeded {QUEUE_ERROR_THRESHOLD}"
            );
            panic!();
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
}

impl Default for Performance {
    fn default() -> Self {
        Self::new()
    }
}

fn dispatch(peers: &mut PeerPerformance, headers: &mut HeaderPerformance, op: PerformanceOp) {
    match op {
        PerformanceOp::RecordIntersection { effect, done } => {
            peers.apply_intersection(effect.peer, effect.current, effect.parent, effect.at);
            signal_done(done);
        }
        PerformanceOp::RecordHeaderAnnouncement { effect, done } => {
            peers.apply_header_announcement(effect.peer, effect.header, effect.parent, effect.at);
            headers.apply_header_received(effect.header, effect.at);
            signal_done(done);
        }
        PerformanceOp::RecordBlocksRequested { effect, done } => {
            headers.apply_blocks_requested(&effect.hashes, effect.requested_at);
            signal_done(done);
        }
        PerformanceOp::RecordBlockDelivery { effect, done } => {
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
            signal_done(done);
        }
        PerformanceOp::RecordFetchFailure { effect, done } => {
            peers.apply_fetch_failure(&effect.peers, effect.at);
            signal_done(done);
        }
        PerformanceOp::RecordKeepaliveRtt { effect, done } => {
            peers.apply_keepalive_rtt(effect.peer, effect.rtt, effect.at);
            signal_done(done);
        }
        PerformanceOp::ClearPeerAvailability { effect, done } => {
            peers.apply_clear_peer_availability(&effect.peer);
            signal_done(done);
        }
        PerformanceOp::ForgetPeer { effect, done } => {
            peers.apply_forget_peer(&effect.peer);
            signal_done(done);
        }
        PerformanceOp::PruneBelow { effect, done } => {
            peers.apply_prune_below(effect.min_height);
            signal_done(done);
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
        PerformanceOp::RecordRollback { effect, done } => {
            peers.apply_rollback(effect.peer, effect.point, effect.parent, effect.at);
            signal_done(done);
        }
        PerformanceOp::RecordHeaderRejected { effect, meter, done } => {
            headers.apply_header_rejected(effect.outcome, meter.as_deref());
            signal_done(done);
        }
        PerformanceOp::RecordHeaderAbandoned { effect, meter, done } => {
            headers.apply_header_abandoned(&effect.hash, effect.now, meter.as_deref());
            signal_done(done);
        }
        PerformanceOp::RecordForkStarted { effect, meter, done } => {
            headers.apply_fork_started(effect.tip, effect.started_at, meter.as_deref());
            signal_done(done);
        }
        PerformanceOp::RecordBlockValid { effect, meter, done } => {
            headers.apply_block_valid(&effect.hash, effect.now, meter.as_deref());
            signal_done(done);
        }
        PerformanceOp::RecordBlockPruned { effect, meter, done } => {
            headers.apply_block_pruned(&effect.hash, effect.invalid, effect.now, meter.as_deref());
            signal_done(done);
        }
    }
}

fn signal_done(done: Option<oneshot::Sender<()>>) {
    if let Some(tx) = done {
        let _ = tx.send(());
    }
}

#[cfg(test)]
mod tests;
