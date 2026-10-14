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
//!
//! Stage-facing operations, who writes them, who reads them, and how urgent that read is.
//! Timeliness classes: C1 header propagation, C2 block fetch, C3 fetch when a peer is lost,
//! C4 chain selection, C5 adversarial disconnect, C7 keep-alive RTT, C8 peer-population
//! maintenance, C9 peer sharing. "Recorded immediately" means the stage enqueues and continues;
//! "awaited" means the stage waits for the worker before it continues; "on demand" means the
//! reader queries at its own decision.
//!
//! | Operation | Writer | Reader | Class | Cadence |
//! | --- | --- | --- | --- | --- |
//! | `record_intersection` | track_peers | fetch selection | C1 | recorded immediately; read on demand |
//! | `record_header_announcement` | track_peers | fetch selection, header lifecycle | C1 | awaited |
//! | `record_rollback` | track_peers | fetch selection | C1 | recorded immediately |
//! | `record_header_rejected` | track_peers | header telemetry | C1 | emitted on the effect path |
//! | `first_announced_at` | query | caller | C1 | on demand |
//! | `record_blocks_requested` | fetch_blocks | header lifecycle | C2 | recorded immediately |
//! | `record_peers_asked` | fetch_blocks | header lifecycle | C2 | awaited |
//! | `record_block_delivery` | fetch_blocks | peer scores, header lifecycle | C2 | awaited |
//! | `record_fetch_failure` | fetch_blocks | peer scores | C2 | recorded immediately |
//! | `select_peers_for_fetch` | fetch_blocks | fetch_blocks | C2 | on demand |
//! | `peer_covers_fragment` | query | caller | C2 | on demand |
//! | `direct_claimants` | query | caller | C2 | on demand |
//! | `record_block_valid` | select_chain | header telemetry | C4 | awaited |
//! | `record_block_pruned` | select_chain | header telemetry | C4 | awaited |
//! | `record_header_abandoned` | select_chain | header telemetry | C4 | awaited |
//! | `record_fork_started` | select_chain | header telemetry | C4 | awaited |
//! | `record_sync_adoption` | adopt_chain | track_peers via `sync_adoption_is_fast` | C4 | recorded immediately |
//! | `sync_adoption_is_fast` | track_peers | track_peers | C4 | on demand |
//! | `prune_below` | adopt_chain | claims and header lifecycles | horizon | awaited |
//! | `peer_adversarial` | peer selection | sharing filters, outbound ranking | C5 | recorded immediately |
//! | `record_keepalive_rtt` | keep-alive initiator | `scores`, `snapshot`, `churn_inputs` | C7 | recorded immediately; read on demand; latest sample and EWMA; does not bump generation |
//! | `record_connection_established` | manager | peer selection | C8 | awaited; the handshake reply follows this write |
//! | `record_connection_closed` | manager | peer selection | C8 | recorded immediately |
//! | `record_connect_failed` | manager | malus, sharing filters | C8 | recorded immediately |
//! | `record_local_use_applied` | connection | peer selection, fetch | C8 | recorded immediately; bumps generation |
//! | `may_initiate` | manager fetch | fetch | C8 | on demand; bearers whose local use is diffusion |
//! | `record_remote_use` | keep-alive, peer-sharing, chain-sync, block-fetch, and tx-submission responders | trace, peer view | C8 | awaited; generation and the span move only when the derived temperature changes |
//! | `record_advertisability` | manager (inside established) and peer selection | sharing filters | C8 | recorded immediately |
//! | `record_connection_failure` | manager (inside connect-failed) | malus, sharing filters, peer selection | C8 | recorded immediately; bumps generation |
//! | `clear_peer_availability` | peer selection, track_peers | fetch selection | C8 | recorded immediately |
//! | `query_peer_view` | peer selection | peer selection | C8 | on demand; `None` when the generation is unchanged; marks are those newer than `since` |
//! | `record_uninteresting` | track_peers | peer selection | C8 | recorded immediately; bumps generation |
//! | `select_outbound` | peer selection | peer selection | C8 | worker copies inputs; caller samples |
//! | `rank_peers_for_churn` | peer selection | peer selection | C8 | worker copies scores; caller ranks |
//! | `set_ledger_candidates` | peer selection | outbound pools | C8 | recorded immediately |
//! | `note_dial` | peer selection | malus half-life | C8 | recorded immediately |
//! | `is_static_peer` | peer selection | churn | C8 | on demand |
//! | `source_counts` | peer selection | peer selection | C8 | on demand |
//! | `evict_records` | peer selection | retention | C8 | recorded immediately, every five minutes; at most 256 entries; does not bump generation |
//! | `select_share_peers` | same sample as `query_share_peers` | peer-sharing reply | C9 | worker copies candidates; caller samples |
//! | `query_share_peers` | peer-sharing responder | peer-sharing reply | C9 | worker copies candidates; caller samples |
//! | `record_shared_peers` | peer-sharing initiator | outbound pools | C9 | awaited; the initiator logs the ingest; newcomers past the learned-address cap are dropped; new candidates bump generation |
//! | `record_share_request_served` | peer-sharing responder | `share_requests` | C9 | recorded immediately; count, last answer, current and previous window; a new row is dropped at the record cap; not enforced |
//! | `ingest_shared_peers` | `record_shared_peers` | outbound pools | C9 | drops newcomers at the learned-address cap; a repeat does not refresh the learned instant |
//! | `scores`, `share_flags`, `snapshot`, `ok_for_sharing`, `shared_contains` | query | caller | — | on demand |
//!
//! Terminal header/fork transitions produce [`HeaderTelemetry`] on the worker; OpenTelemetry
//! events and metrics are emitted only on the external-effect path so export drops or lag cannot
//! stall or couple to performance state updates.

mod adoption;
mod effects;
mod header;
mod ops;
mod peer_tracking;
mod peers;
mod spans;

use std::{
    fmt,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use adoption::SyncAdoptionPace;
use amaru_kernel::PeerCandidate;
use amaru_observability::{error, warn};
pub use effects::*;
pub use header::{ForkSwitchOutcome, HeaderLifecycleOutcome, HeaderPerformance, HeaderTelemetry};
use ops::PerformanceOp;
use parking_lot::Mutex;
pub use peers::{
    ADVERSARIAL_IMPULSE, BAN_STUB_GRACE, BlockClaim, CONNECT_FAIL_IMPULSE, ChurnInput, ChurnRank, ClaimKind,
    DEFAULT_MALUS_HALF_LIFE, DEFAULT_PEER_MALUS_HALF_LIFE, DEFAULT_PEERS_MIX, DialOutcome, EVICTION_BATCH, EvictBatch,
    FetchPeerSet, MixEntry, NEVER_CONNECTED_BONUS, OutboundInputs, OutboundPick, PEER_RECORD_CAP,
    PEER_RECORD_RETENTION, PeerMix, PeerMixParseError, PeerPerformance, PeerScores, PeerShareFlags, PeerSnapshot,
    PeerSource, PeerView, SHARE_MALUS_THRESHOLD, SHARE_POLICY_MAX, SHARE_REQUEST_WINDOW, SHARED_PEERS_CAP,
    SelectOutboundParams, SelectPeersParams, SelectUsing, ShareRequestRecord, SharedIngestResult, SourceCounts,
    UninterestingMark, ViewConnection, malus_at,
};
use spans::HeaderSpanBook;
#[doc(hidden)]
pub use spans::close_root_forward_after_exit;
use tokio::{
    sync::mpsc::{UnboundedSender, unbounded_channel},
    time::Instant as TokioInstant,
};
#[cfg(test)]
use {ops::is_urgent_op, std::sync::atomic::AtomicU64};

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

impl WorkerGuard {
    fn join(&self) -> thread::Result<()> {
        self.join.lock().take().map_or(Ok(()), JoinHandle::join)
    }
}

impl Drop for WorkerGuard {
    fn drop(&mut self) {
        if let Err(payload) = self.join() {
            let error = payload
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| payload.downcast_ref::<&str>().copied())
                .unwrap_or("unknown panic payload");
            error!(consensus::performance::WORKER_PANICKED, error);
        }
    }
}

/// Handle to the performance subsystem (Send + Sync). State lives on a worker thread.
///
/// Field order matters for cleanup: `tx` is dropped before the worker join guard, so the last
/// sender closes the op channel and the worker can exit before the join runs.
///
/// Dropping the last clone joins the worker and waits while it drains any remaining ops,
/// unless a retained [`Self::shutdown_callback`] owns that join.
struct Job {
    op: PerformanceOp,
    #[cfg(test)]
    queued_at: std::time::Instant,
}

#[cfg(test)]
struct Probe(Option<Arc<QueueProbe>>);
#[cfg(not(test))]
struct Probe;

impl Probe {
    fn none() -> Self {
        #[cfg(test)]
        {
            Self(None)
        }
        #[cfg(not(test))]
        {
            Self
        }
    }

    #[cfg(test)]
    fn hold(probe: Arc<QueueProbe>) -> Self {
        Self(Some(probe))
    }
}

#[expect(clippy::expect_used)]
fn spawn_worker(
    mut rx: tokio::sync::mpsc::UnboundedReceiver<Job>,
    pending: Arc<AtomicUsize>,
    initial_peers: PeerPerformance,
    probe: Probe,
) -> JoinHandle<()> {
    thread::Builder::new()
        .name("performance".into())
        .spawn(move || {
            #[cfg(not(test))]
            let _probe = probe;
            #[cfg(test)]
            let probe = {
                if let Some(probe) = probe.0 {
                    if let Some(gate) = probe.release.lock().take() {
                        let _ = gate.recv();
                    }
                    *probe.ready_at.lock() = Some(std::time::Instant::now());
                    Some(probe)
                } else {
                    None
                }
            };
            let rt =
                tokio::runtime::Builder::new_current_thread().enable_all().build().expect("performance worker runtime");
            rt.block_on(async move {
                let mut peers = initial_peers;
                let mut headers = HeaderPerformance::new();
                let mut pace = SyncAdoptionPace::default();
                while let Some(job) = rx.recv().await {
                    pending.fetch_sub(1, Ordering::Relaxed);
                    #[cfg(test)]
                    if let Some(probe) = probe.as_ref()
                        && is_urgent_op(&job.op)
                    {
                        let ready = *probe.ready_at.lock();
                        let basis = ready.map(|ready| job.queued_at.max(ready)).unwrap_or(job.queued_at);
                        let wait = std::time::Instant::now().saturating_duration_since(basis);
                        let nanos = u64::try_from(wait.as_nanos()).unwrap_or(u64::MAX);
                        probe.max_urgent_ns.fetch_max(nanos, Ordering::Relaxed);
                    }
                    ops::dispatch(&mut peers, &mut headers, &mut pace, job.op);
                }
            });
        })
        .expect("failed to spawn performance worker thread")
}

/// Records how long urgent ops waited in the worker queue after the worker was ready.
///
/// The gate holds the worker before its receive loop so a test can fill the queue first.
/// Wait is measured from `max(enqueue time, worker-ready time)`, so the artificial pause
/// is not included. This is the `instrumented-worker-queue-wait` measurement.
#[cfg(test)]
pub struct QueueProbe {
    release: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
    ready_at: Mutex<Option<std::time::Instant>>,
    max_urgent_ns: AtomicU64,
}

#[cfg(test)]
impl QueueProbe {
    pub fn max_urgent_wait(&self) -> Duration {
        Duration::from_nanos(self.max_urgent_ns.load(Ordering::Relaxed))
    }
}

pub struct Performance {
    tx: UnboundedSender<Job>,
    /// Approximate number of ops queued or being processed (incremented before send).
    pending: Arc<AtomicUsize>,
    /// Monotonic time of the last queue-depth WARN (for 1/s rate limiting).
    ///
    /// Uses [`tokio::time::Instant`] rather than [`std::time::Instant`]: on some platforms a
    /// buggy monotonic clock can go backwards and make `std` panics in `duration_since`, while
    /// Tokio's Instant is safe to read outside a runtime (see unit test) and saturates.
    last_queue_warn: Arc<Mutex<Option<TokioInstant>>>,
    worker: Arc<WorkerGuard>,
    /// Open header spans. The worker never touches this map.
    pub(crate) spans: Arc<Mutex<HeaderSpanBook>>,
}

impl Clone for Performance {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
            pending: Arc::clone(&self.pending),
            last_queue_warn: Arc::clone(&self.last_queue_warn),
            worker: Arc::clone(&self.worker),
            spans: Arc::clone(&self.spans),
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

impl Performance {
    /// Start the performance worker thread and return a handle to enqueue operations.
    ///
    /// Dropping the last clone of this handle closes the op channel and joins the worker thread
    /// (after it drains remaining ops). Prefer that drop on a non-hot path (node teardown), not
    /// from a multi-thread Tokio worker task.
    pub fn new() -> Self {
        Self::with_peer_sources(Default::default(), Default::default(), Default::default(), PeerMix::default())
    }

    /// Start the worker with outbound candidate sources and mix.
    ///
    /// This is the **only** place static/snapshot/ledger pools and the peer-mix formula are set;
    /// there is no live reconfiguration effect. Pools are [`PeerCandidate`]s; Host/SRV names are
    /// resolved on demand each time they are selected.
    pub fn with_peer_sources(
        static_peers: std::collections::BTreeSet<PeerCandidate>,
        snapshot_candidates: std::collections::BTreeSet<PeerCandidate>,
        ledger_candidates: std::collections::BTreeSet<PeerCandidate>,
        peer_mix: PeerMix,
    ) -> Self {
        let (tx, rx) = unbounded_channel::<Job>();
        let pending = Arc::new(AtomicUsize::new(0));
        let initial_peers =
            PeerPerformance::with_sources(static_peers, snapshot_candidates, ledger_candidates, peer_mix);
        let join = spawn_worker(rx, Arc::clone(&pending), initial_peers, Probe::none());

        Self {
            tx,
            pending,
            last_queue_warn: Arc::new(Mutex::new(None)),
            worker: Arc::new(WorkerGuard { join: Mutex::new(Some(join)) }),
            spans: Arc::new(Mutex::new(HeaderSpanBook::default())),
        }
    }

    /// Worker that waits on `release` before receiving, so a test can fill the queue first.
    ///
    /// Urgent-op wait is [`QueueProbe::max_urgent_wait`]. Named `instrumented-worker-queue-wait`.
    #[cfg(test)]
    pub fn paused() -> (Self, std::sync::mpsc::Sender<()>, Arc<QueueProbe>) {
        Self::paused_with(PeerPerformance::new())
    }

    /// Same as [`Self::paused`], starting from `initial` so a cap-sized map is already in place.
    #[cfg(test)]
    pub fn paused_with(initial: PeerPerformance) -> (Self, std::sync::mpsc::Sender<()>, Arc<QueueProbe>) {
        let (tx, rx) = unbounded_channel::<Job>();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let probe = Arc::new(QueueProbe {
            release: Mutex::new(Some(release_rx)),
            ready_at: Mutex::new(None),
            max_urgent_ns: AtomicU64::new(0),
        });
        let pending = Arc::new(AtomicUsize::new(0));
        let join = spawn_worker(rx, Arc::clone(&pending), initial, Probe::hold(Arc::clone(&probe)));
        (
            Self {
                tx,
                pending,
                last_queue_warn: Arc::new(Mutex::new(None)),
                worker: Arc::new(WorkerGuard { join: Mutex::new(Some(join)) }),
                spans: Arc::new(Mutex::new(HeaderSpanBook::default())),
            },
            release_tx,
            probe,
        )
    }

    /// Retain a worker join callback without keeping its request channel open.
    ///
    /// Drop all `Performance` handles before invoking this callback. It waits for the
    /// worker to finish and returns its panic instead of logging it during drop.
    pub fn shutdown_callback(&self) -> impl FnOnce() -> thread::Result<()> + Send + Sync + 'static {
        let worker = Arc::clone(&self.worker);
        move || worker.join()
    }

    /// Enqueue an operation. Updates the pending counter and logs WARN/ERROR thresholds.
    pub(crate) fn submit(&self, op: PerformanceOp) {
        let depth = self.pending.fetch_add(1, Ordering::Relaxed) + 1;
        if depth > QUEUE_WARN_THRESHOLD && self.should_log_queue_warn() {
            warn!(consensus::performance::QUEUE_LAGGING, queue_depth = depth as u64);
        }
        #[expect(clippy::panic)]
        if depth == QUEUE_ERROR_THRESHOLD + 1 {
            error!(
                consensus::performance::QUEUE_OVERFLOW,
                queue_depth = depth as u64,
                threshold = QUEUE_ERROR_THRESHOLD as u64
            );
            // NOTE: Amaru fails loudly and early when design assumptions are dynamically
            // violated. The performance worker is expected to keep pace with consensus stages;
            // an unbounded queue past this depth means that assumption no longer holds, so we
            // panic rather than silently drop telemetry or grow without bound.
            panic!("performance op queue exceeded {QUEUE_ERROR_THRESHOLD}");
        }
        // If the worker has died, drop the op; the pending counter will be slightly wrong.
        let job = Job {
            op,
            #[cfg(test)]
            queued_at: std::time::Instant::now(),
        };
        if self.tx.send(job).is_err() {
            self.pending.fetch_sub(1, Ordering::Relaxed);
        }
    }

    /// Approximate number of ops queued or in flight.
    pub fn queue_depth(&self) -> usize {
        self.pending.load(Ordering::Relaxed)
    }

    /// Queue a ledger-candidate replacement on this worker. Tests use it to bump the generation
    /// the same way the ledger-check child does.
    #[cfg(test)]
    pub(crate) fn testing_replace_ledger_candidates(&self, candidates: std::collections::BTreeSet<PeerCandidate>) {
        self.submit(ops::PerformanceOp::Peer(ops::PeerOp::SetLedgerCandidates {
            effect: SetLedgerCandidatesEffect { candidates },
        }));
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

#[cfg(test)]
mod tests;
