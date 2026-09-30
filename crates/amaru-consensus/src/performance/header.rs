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

//! Per-header lifecycle timing for network-health events and metrics.
//!
//! State transitions are pure: methods return [`HeaderTelemetry`] payloads. OpenTelemetry events
//! and metrics are **not** emitted here — the performance worker must not run export paths that
//! may drop or block under resource/connectivity pressure. Callers (external-effect handlers on
//! the stage executor) invoke `HeaderTelemetry::emit`.

use std::{collections::BTreeMap, time::Duration};

use amaru_kernel::{BlockHeight, HeaderHash, Peer, Point};
use amaru_metrics::{Meter, MetricRecorder, consensus::ConsensusMetrics};
use amaru_observability::{debug, error, info};
use amaru_pure_stage::Instant;

use super::spans::HeaderSpanBook;

/// How many distinct announcing peers are logged for one header hash.
const LOGGED_ANNOUNCEMENTS: usize = 3;

/// Tracks the processing of headers to produce a single `perf.header.lifecycle` telemetry payload
/// per header when its block reaches a terminal state (adopted, invalidated, abandoned, or pruned).
/// The payload covers the virtual slot start followed by the four network-health processing points
/// of a header's lifecycle and the intervals between them.
///
/// The points themselves are emitted as they happen (`header.announced`, `block.received`,
/// `block.adopted` on target `amaru::blockperf`). `block.requested` is emitted by the fetch stage
/// once the contacted peers are known.
///
/// - `slot_start_to_header_micros`: from the virtual beginning of the slot to the header's
///   reception (computed by stages via era history; omitted when the node is still syncing).
/// - `block_fetch_wait_micros`: from the header's reception to the request of its block.
/// - `block_fetch_micros`: from the request of the block to its reception.
/// - `forward_micros`: from the header's reception to the adoption of its block.
///
/// The first peer that announced the header is retained so terminal lifecycle events can attribute
/// timings to that peer (TUI / `perf.header.lifecycle`). Later announcements of the same hash do not
/// overwrite the first announcer or the slot-start interval.
///
/// Fork switches are tracked independently and surface as `perf.fork.switch` telemetry.
///
/// Owned by the performance worker thread; emission of returned telemetry happens off that thread.
#[derive(Debug, Default)]
pub struct HeaderPerformance {
    /// The lifecycle timestamps of each header whose block has not yet reached a terminal state.
    lifecycles: BTreeMap<HeaderHash, HeaderLifecycle>,
    /// Announcing and delivering peers kept until the header falls behind the immutable horizon.
    /// Announcements are logged only for the first `LOGGED_ANNOUNCEMENTS` distinct peers, and only
    /// while this map still holds the hash. An already-stored header does not open a new entry.
    /// Once the header has been adopted, further announcements are not logged.
    audience: BTreeMap<HeaderHash, HeaderAudience>,
    /// An in-progress fork switch, if any.
    fork_switch: Option<ForkSwitch>,
}

/// Peers that have announced or delivered one header, in arrival order.
#[derive(Debug, Clone)]
struct HeaderAudience {
    height: BlockHeight,
    announced: Vec<Peer>,
    delivered: Vec<Peer>,
    /// The block was adopted. Later peers do not emit `header.announced`.
    adopted: bool,
    /// Slot onset as a duration since the global epoch, from the first announcement that knew it.
    slot_onset: Option<Duration>,
    /// When each peer was asked for this body. The first ask of a peer wins.
    asked_at: BTreeMap<Peer, Instant>,
}

/// The processing timestamps accumulated for a header until its block reaches a terminal state.
#[derive(Debug, Clone)]
struct HeaderLifecycle {
    /// Peer from which the header was first received.
    peer: Peer,
    /// Interval from virtual slot start to header reception, computed by the announcing stage.
    slot_start_to_header_micros: u64,
    /// Block height of the header (for immutable-horizon pruning).
    height: BlockHeight,
    /// Time when the header was first received from an upstream peer.
    received_at: Instant,
    /// Time when its block was first requested, if it was requested.
    requested_at: Option<Instant>,
    /// Time when its block was first received, if it was received.
    downloaded_at: Option<Instant>,
}

impl HeaderLifecycle {
    fn new(peer: Peer, slot_start_to_header_micros: u64, height: BlockHeight, received_at: Instant) -> Self {
        Self { peer, slot_start_to_header_micros, height, received_at, requested_at: None, downloaded_at: None }
    }
}

/// The in-progress switch to a new fork, with the hash of its expected new best tip and the
/// time it was detected.
#[derive(Debug, Clone)]
struct ForkSwitch {
    hash: HeaderHash,
    started_at: Instant,
}

/// Local enum modelling the outcome of a header's lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum HeaderLifecycleOutcome {
    /// We stopped trying to get the validity of a block (it might still have been downloaded in the
    /// meantime).
    AbandonedBlock,
    /// The block was retrieved but validation failed.
    InvalidBlock,
    /// The block was retrieved and validated.
    ValidBlock,
    /// Tracking was dropped because the header fell outside the retained consensus window
    /// (horizon / immutable tip).
    Pruned,
    /// The header for a given block has already been received.
    DuplicateHeader,
    /// The header cannot be decoded.
    UndecodableHeader,
    /// The header is invalid.
    InvalidHeader,
    /// The header for that block could not be stored.
    StoreHeaderError,
}

impl HeaderLifecycleOutcome {
    /// Header rejected before a lifecycle was tracked. Logged at error, with no durations.
    pub fn is_reception_rejection(self) -> bool {
        matches!(self, Self::DuplicateHeader | Self::UndecodableHeader | Self::InvalidHeader | Self::StoreHeaderError)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::AbandonedBlock => "abandoned",
            Self::InvalidBlock => "invalid",
            Self::ValidBlock => "valid",
            Self::Pruned => "pruned",
            Self::DuplicateHeader => "duplicate_header",
            Self::InvalidHeader => "invalid_header",
            Self::UndecodableHeader => "undecodable_header",
            Self::StoreHeaderError => "store_header_error",
        }
    }
}

/// Local enum modelling the outcome of the `perf.fork.switch` event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ForkSwitchOutcome {
    /// We stopped trying to get the validity of a block (it might still have been downloaded in the
    /// meantime).
    AbandonedBlock,
    /// The block was retrieved but validation failed.
    InvalidBlock,
    /// The block was retrieved and validated.
    ValidBlock,
    /// This fork has been superseded by a better one.
    Superseded,
}

impl ForkSwitchOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AbandonedBlock => "abandoned",
            Self::InvalidBlock => "invalid",
            Self::ValidBlock => "valid",
            Self::Superseded => "superseded_fork",
        }
    }
}

/// Telemetry produced by a header/fork state transition.
///
/// Emit via `HeaderTelemetry::emit` on the external-effect path (not on the performance worker).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeaderTelemetry {
    /// Closed or rejected header lifecycle (`perf.header.lifecycle`).
    Lifecycle {
        hash: Option<HeaderHash>,
        /// First announcer peer when a tracked lifecycle is closed; `None` for reception rejections.
        peer: Option<Peer>,
        outcome: HeaderLifecycleOutcome,
        /// Omitted while syncing or when no tracked lifecycle exists.
        slot_start_to_header_micros: Option<u64>,
        block_fetch_wait_micros: Option<u64>,
        block_fetch_micros: Option<u64>,
        forward_micros: Option<u64>,
        /// Body reception to adoption. Not `forward − wait − fetch`.
        adopt_micros: Option<u64>,
        /// Present when a header is rejected on reception.
        error: Option<String>,
    },
    /// Closed fork switch (`perf.fork.switch`).
    ForkSwitch { hash: HeaderHash, outcome: ForkSwitchOutcome, duration_micros: u64 },
    /// One of the first three distinct peers to announce `hash` (`header.announced`).
    Announced { hash: HeaderHash, peer: Peer, rank: u64, slot_latency_ms: Option<u64> },
    /// Peers asked for the body of `hash` (`block.requested`).
    Requested { hash: HeaderHash, peers: String, slot_latency_ms: Option<u64> },
    /// A distinct peer delivered the body of `hash` (`block.received`).
    Received {
        hash: HeaderHash,
        peer: Peer,
        rank: u64,
        slot_latency_ms: Option<u64>,
        /// Milliseconds since this peer was asked. Absent when that ask was not recorded.
        fetch_latency_ms: Option<u64>,
    },
    /// `hash` was adopted locally (`block.adopted`).
    Adopted { hash: HeaderHash, peer: Option<Peer>, slot_latency_ms: Option<u64> },
}

impl HeaderTelemetry {
    /// Emit the corresponding tracing event and optional metric.
    ///
    /// Safe to call where OTel/export layers may drop or lag; must not run on the performance
    /// worker thread. Blockperf lines are emitted under the forward span when it is still open.
    /// Lifecycle closes that span and uses its duration.
    pub(crate) fn emit(&self, meter: Option<&Meter>, live: bool, spans: &mut HeaderSpanBook) {
        match self {
            Self::Lifecycle {
                hash,
                peer,
                outcome,
                error,
                slot_start_to_header_micros,
                block_fetch_wait_micros,
                block_fetch_micros,
                forward_micros,
                adopt_micros,
            } => {
                let recorded =
                    hash.and_then(|hash| spans.finish(&hash, *outcome == HeaderLifecycleOutcome::ValidBlock));
                let block_fetch_wait_micros = recorded
                    .as_ref()
                    .and_then(|durations| durations.block_fetch_wait_micros)
                    .or(*block_fetch_wait_micros);
                let block_fetch_micros =
                    recorded.as_ref().and_then(|durations| durations.block_fetch_micros).or(*block_fetch_micros);
                let forward_micros =
                    recorded.as_ref().and_then(|durations| durations.forward_micros).or(*forward_micros);
                let adopt_micros = recorded.as_ref().and_then(|durations| durations.adopt_micros).or(*adopt_micros);
                let rejected = outcome.is_reception_rejection();
                emit_lifecycle(
                    rejected,
                    *peer,
                    *hash,
                    outcome.as_str(),
                    error.as_deref(),
                    *slot_start_to_header_micros,
                    block_fetch_wait_micros,
                    block_fetch_micros,
                    forward_micros,
                    adopt_micros,
                );
                record_metric(
                    meter,
                    ConsensusMetrics::HeaderLifecycle {
                        outcome: outcome.as_str().to_string(),
                        slot_start_to_header_micros: *slot_start_to_header_micros,
                        block_fetch_wait_micros,
                        block_fetch_micros,
                        forward_micros,
                    },
                );
            }
            Self::ForkSwitch { hash, outcome, duration_micros } => {
                let duration_micros = spans.take_fork_micros().unwrap_or(*duration_micros);
                debug!(
                    consensus::perf::fork::SWITCH,
                    header_hash = hash,
                    outcome = outcome.as_str(),
                    duration_micros = duration_micros
                );
                record_metric(
                    meter,
                    ConsensusMetrics::ForkSwitch { outcome: outcome.as_str().to_string(), duration_micros },
                );
            }
            Self::Announced { hash, peer, rank, slot_latency_ms } => {
                in_forward(spans, hash, || {
                    if live {
                        info!(
                            blockperf::header::ANNOUNCED,
                            peer,
                            header_hash = hash,
                            rank = *rank,
                            slot_latency_ms = @slot_latency_ms
                        );
                    } else {
                        debug!(
                            blockperf::header::ANNOUNCED,
                            peer,
                            header_hash = hash,
                            rank = *rank,
                            slot_latency_ms = @slot_latency_ms
                        );
                    }
                });
            }
            Self::Requested { hash, peers, slot_latency_ms } => {
                in_forward(spans, hash, || {
                    if live {
                        info!(
                            blockperf::block::REQUESTED,
                            header_hash = hash,
                            peers = peers.as_str(),
                            slot_latency_ms = @slot_latency_ms
                        );
                    } else {
                        debug!(
                            blockperf::block::REQUESTED,
                            header_hash = hash,
                            peers = peers.as_str(),
                            slot_latency_ms = @slot_latency_ms
                        );
                    }
                });
            }
            Self::Received { hash, peer, rank, slot_latency_ms, fetch_latency_ms } => {
                in_forward(spans, hash, || {
                    if live {
                        info!(
                            blockperf::block::RECEIVED,
                            peer,
                            header_hash = hash,
                            rank = *rank,
                            slot_latency_ms = @slot_latency_ms,
                            fetch_latency_ms = @fetch_latency_ms
                        );
                    } else {
                        debug!(
                            blockperf::block::RECEIVED,
                            peer,
                            header_hash = hash,
                            rank = *rank,
                            slot_latency_ms = @slot_latency_ms,
                            fetch_latency_ms = @fetch_latency_ms
                        );
                    }
                });
            }
            Self::Adopted { hash, peer: Some(peer), slot_latency_ms } => {
                in_forward(spans, hash, || {
                    if live {
                        info!(blockperf::block::ADOPTED, header_hash = hash, peer, slot_latency_ms = @slot_latency_ms);
                    } else {
                        debug!(blockperf::block::ADOPTED, header_hash = hash, peer, slot_latency_ms = @slot_latency_ms);
                    }
                });
            }
            Self::Adopted { hash, peer: None, slot_latency_ms } => {
                in_forward(spans, hash, || {
                    if live {
                        info!(blockperf::block::ADOPTED, header_hash = hash, slot_latency_ms = @slot_latency_ms);
                    } else {
                        debug!(blockperf::block::ADOPTED, header_hash = hash, slot_latency_ms = @slot_latency_ms);
                    }
                });
            }
        }
    }

    /// Emit a batch of telemetry events.
    ///
    /// Blockperf and fork events run before lifecycle so they still see the forward span.
    /// `live` prints block-propagation events at info; sync keeps them at debug.
    pub(crate) fn emit_all(events: &[Self], meter: Option<&Meter>, live: bool, spans: &mut HeaderSpanBook) {
        for event in events.iter().filter(|event| !matches!(event, Self::Lifecycle { .. })) {
            event.emit(meter, live, spans);
        }
        for event in events.iter().filter(|event| matches!(event, Self::Lifecycle { .. })) {
            event.emit(meter, live, spans);
        }
    }

    /// Rejected header with no tracked lifecycle (no durations).
    pub fn rejected(outcome: HeaderLifecycleOutcome) -> Self {
        Self::rejected_with(outcome, None, None, None)
    }

    pub fn rejected_with(
        outcome: HeaderLifecycleOutcome,
        peer: Option<Peer>,
        hash: Option<HeaderHash>,
        error: Option<String>,
    ) -> Self {
        Self::Lifecycle {
            hash,
            peer,
            outcome,
            error,
            slot_start_to_header_micros: None,
            block_fetch_wait_micros: None,
            block_fetch_micros: None,
            forward_micros: None,
            adopt_micros: None,
        }
    }
}

fn in_forward(spans: &HeaderSpanBook, hash: &HeaderHash, emit: impl FnOnce()) {
    if let Some(span) = spans.forward_span(hash) {
        let _entered = span.enter();
        emit();
    } else {
        emit();
    }
}

#[expect(clippy::too_many_arguments)]
fn emit_lifecycle(
    rejected: bool,
    peer: Option<Peer>,
    hash: Option<HeaderHash>,
    outcome: &str,
    error_text: Option<&str>,
    slot_start_to_header_micros: Option<u64>,
    block_fetch_wait_micros: Option<u64>,
    block_fetch_micros: Option<u64>,
    forward_micros: Option<u64>,
    adopt_micros: Option<u64>,
) {
    macro_rules! lifecycle {
        ($macro:ident $($field:tt)*) => {{
            $macro!(
                consensus::perf::header::LIFECYCLE,
                $($field)*
                outcome = outcome,
                error = @error_text,
                slot_start_to_header_micros = @slot_start_to_header_micros,
                block_fetch_wait_micros = @block_fetch_wait_micros,
                block_fetch_micros = @block_fetch_micros,
                forward_micros = @forward_micros,
                adopt_micros = @adopt_micros
            );
        }};
    }
    match (rejected, peer, hash) {
        (true, Some(peer), Some(hash)) => lifecycle!(error peer, header_hash = hash,),
        (true, Some(peer), None) => lifecycle!(error peer,),
        (true, None, Some(hash)) => lifecycle!(error header_hash = hash,),
        (true, None, None) => lifecycle!(error),
        (false, Some(peer), Some(hash)) => lifecycle!(debug peer, header_hash = hash,),
        (false, Some(peer), None) => lifecycle!(debug peer,),
        (false, None, Some(hash)) => lifecycle!(debug header_hash = hash,),
        (false, None, None) => lifecycle!(debug),
    }
}

impl HeaderPerformance {
    pub fn new() -> Self {
        Self::default()
    }

    /// A header has been accepted from upstream: start tracking its lifecycle from `received_at`.
    /// Subsequent announcements of the same header do not move `received_at`, the first announcer
    /// peer, or the precomputed slot-start interval (first wins).
    ///
    /// Returns `header.announced` telemetry for a peer that is among the first three
    /// distinct announcers of this hash.
    ///
    /// When `already_stored` is true, the announcement is logged only if this hash is still
    /// collecting announcers. It does not open a rank-1 line or a new lifecycle: a slow peer
    /// repeating headers that are already in the store stays quiet.
    ///
    /// `slot_start_to_header_micros` and `slot_onset` are computed by the caller (using era
    /// history) so this type stays free of consensus calendar knowledge. The micros are recorded
    /// only for a header that is not already stored. `slot_onset` is the slot start as a duration
    /// since the global epoch, kept so later blockperf lines can report milliseconds since then.
    pub fn apply_header_received(
        &mut self,
        peer: Peer,
        tip: Point,
        received_at: Instant,
        slot_start_to_header_micros: u64,
        slot_onset: Duration,
        already_stored: bool,
    ) -> Vec<HeaderTelemetry> {
        let hash = tip.hash();
        if already_stored {
            return self.extend_announcement(peer, hash, received_at, slot_onset).into_iter().collect();
        }
        self.lifecycles.entry(hash).or_insert_with(|| {
            HeaderLifecycle::new(peer, slot_start_to_header_micros, tip.block_height(), received_at)
        });
        self.note_announcement(peer, hash, tip.block_height(), received_at, slot_onset).into_iter().collect()
    }

    /// Close open lifecycles (and any matching fork switch) for headers that have fallen behind
    /// the immutable horizon (`height < min_height`). Returns `Pruned` (and fork) telemetry.
    pub fn apply_prune_below(&mut self, min_height: BlockHeight, now: Instant) -> Vec<HeaderTelemetry> {
        let to_prune: Vec<HeaderHash> =
            self.lifecycles.iter().filter(|(_, lc)| lc.height < min_height).map(|(h, _)| *h).collect();
        let mut out = Vec::new();
        for hash in to_prune {
            // Not a sync-path decision; always include the slot-start interval when available.
            out.extend(self.close_lifecycle(&hash, HeaderLifecycleOutcome::Pruned, now, false));
            out.extend(self.close_fork(&hash, ForkSwitchOutcome::AbandonedBlock, now));
        }
        self.audience.retain(|_, audience| audience.height >= min_height);
        out
    }

    /// These peers were asked for these bodies at `at`.
    ///
    /// The first ask of a given peer is kept, so a later staggered ask of a new peer gets its own
    /// timestamp and a repeat ask does not move the original. Returns one `block.requested` line
    /// per header.
    pub fn apply_peers_asked(&mut self, hashes: &[HeaderHash], peers: &[Peer], at: Instant) -> Vec<HeaderTelemetry> {
        let mut ordered = peers.to_vec();
        ordered.sort();
        ordered.dedup();
        let peers_field = ordered.iter().map(|peer| peer.to_string()).collect::<Vec<_>>().join(",");
        hashes
            .iter()
            .map(|hash| {
                let audience = self.audience_mut(*hash, BlockHeight::from(0));
                for peer in &ordered {
                    audience.asked_at.entry(*peer).or_insert(at);
                }
                let slot_latency_ms = audience.slot_onset.map(|onset| latency_since_onset(onset, at));
                HeaderTelemetry::Requested { hash: *hash, peers: peers_field.clone(), slot_latency_ms }
            })
            .collect()
    }

    /// The fetch stage requested the blocks for these headers: record their request time.
    pub fn apply_blocks_requested(&mut self, hashes: &[HeaderHash], requested_at: Instant) {
        for hash in hashes {
            if let Some(lifecycle) = self.lifecycles.get_mut(hash) {
                lifecycle.requested_at.get_or_insert(requested_at);
            }
        }
    }

    /// The fetch stage received a block for this header: record the first reception time and,
    /// for each distinct delivering peer, return `block.received` telemetry with arrival rank.
    pub fn apply_block_downloaded(
        &mut self,
        peer: Peer,
        hash: &HeaderHash,
        height: BlockHeight,
        downloaded_at: Instant,
    ) -> Vec<HeaderTelemetry> {
        if let Some(lifecycle) = self.lifecycles.get_mut(hash) {
            lifecycle.downloaded_at.get_or_insert(downloaded_at);
        }
        self.note_delivery(peer, *hash, height, downloaded_at).into_iter().collect()
    }

    /// Header rejected on reception (duplicate, undecodable, invalid, store error, …).
    /// Does not require a tracked lifecycle; telemetry has no duration fields.
    pub fn apply_header_rejected(outcome: HeaderLifecycleOutcome) -> HeaderTelemetry {
        HeaderTelemetry::rejected(outcome)
    }

    /// A received header that is abandoned because its block depends on an invalid block.
    pub fn apply_header_abandoned(&mut self, hash: &HeaderHash, now: Instant) -> Vec<HeaderTelemetry> {
        // Not a sync-path decision; always include the slot-start interval when available.
        self.close_lifecycle(hash, HeaderLifecycleOutcome::AbandonedBlock, now, false)
    }

    /// A fork has been detected: start tracking the time it takes to switch to it. If a previous
    /// fork switch is still in progress, return it as superseded telemetry.
    pub fn apply_fork_started(&mut self, tip: Point, started_at: Instant) -> Vec<HeaderTelemetry> {
        let mut out = Vec::new();
        if let Some(previous) = self.fork_switch.take() {
            out.push(fork_telemetry(&previous.hash, ForkSwitchOutcome::Superseded, started_at, previous.started_at));
        }
        self.fork_switch = Some(ForkSwitch { hash: tip.hash(), started_at });
        out
    }

    /// The block for a header has been validated and adopted.
    ///
    /// When `syncing` is true, `slot_start_to_header_micros` is omitted (not meaningful while
    /// catching up far behind the network tip).
    pub fn apply_block_valid(&mut self, hash: &HeaderHash, now: Instant, syncing: bool) -> Vec<HeaderTelemetry> {
        let (delivered_by, slot_latency_ms) = match self.audience.get_mut(hash) {
            Some(audience) => {
                audience.adopted = true;
                (audience.delivered.first().copied(), audience.slot_onset.map(|onset| latency_since_onset(onset, now)))
            }
            None => (None, None),
        };
        let mut out = self.close_lifecycle(hash, HeaderLifecycleOutcome::ValidBlock, now, syncing);
        out.push(HeaderTelemetry::Adopted { hash: *hash, peer: delivered_by, slot_latency_ms });
        out.extend(self.close_fork(hash, ForkSwitchOutcome::ValidBlock, now));
        out
    }

    /// A header has been pruned after a block validation.
    ///
    /// `invalid == true` means that the block was found invalid; otherwise this header/block
    /// has been abandoned because a better chain is available.
    ///
    /// When `syncing` is true, `slot_start_to_header_micros` is omitted.
    pub fn apply_block_pruned(
        &mut self,
        hash: &HeaderHash,
        invalid: bool,
        now: Instant,
        syncing: bool,
    ) -> Vec<HeaderTelemetry> {
        let (header_outcome, fork_outcome) = if invalid {
            (HeaderLifecycleOutcome::InvalidBlock, ForkSwitchOutcome::InvalidBlock)
        } else {
            (HeaderLifecycleOutcome::AbandonedBlock, ForkSwitchOutcome::AbandonedBlock)
        };
        let mut out = self.close_lifecycle(hash, header_outcome, now, syncing);
        out.extend(self.close_fork(hash, fork_outcome, now));
        out
    }

    /// Number of headers currently tracked (tests / diagnostics).
    pub fn lifecycle_count(&self) -> usize {
        self.lifecycles.len()
    }

    /// First peer that announced `hash`, if tracked (tests / diagnostics).
    pub fn first_announcer(&self, hash: &HeaderHash) -> Option<Peer> {
        self.lifecycles.get(hash).map(|l| l.peer)
    }

    /// Stage-computed slot-start interval for `hash`, if tracked (tests / diagnostics).
    pub fn slot_start_to_header_micros(&self, hash: &HeaderHash) -> Option<u64> {
        self.lifecycles.get(hash).map(|l| l.slot_start_to_header_micros)
    }

    /// Whether a fork switch is in progress for `hash` (tests / diagnostics).
    pub fn has_fork_switch(&self, hash: &HeaderHash) -> bool {
        self.fork_switch.as_ref().is_some_and(|f| &f.hash == hash)
    }
}

impl HeaderPerformance {
    fn note_announcement(
        &mut self,
        peer: Peer,
        hash: HeaderHash,
        height: BlockHeight,
        at: Instant,
        slot_onset: Duration,
    ) -> Option<HeaderTelemetry> {
        let audience = self.audience_mut(hash, height);
        audience.slot_onset.get_or_insert(slot_onset);
        Self::push_announcer(audience, peer, hash, at)
    }

    /// Rank 2 or 3 for a header whose announcement list is still open.
    ///
    /// Missing audience means the header was already stored before we started collecting, or it
    /// has since fallen off the horizon. Either way there is no rank-1 line to emit.
    fn extend_announcement(
        &mut self,
        peer: Peer,
        hash: HeaderHash,
        at: Instant,
        slot_onset: Duration,
    ) -> Option<HeaderTelemetry> {
        let audience = self.audience.get_mut(&hash)?;
        audience.slot_onset.get_or_insert(slot_onset);
        Self::push_announcer(audience, peer, hash, at)
    }

    fn push_announcer(
        audience: &mut HeaderAudience,
        peer: Peer,
        hash: HeaderHash,
        at: Instant,
    ) -> Option<HeaderTelemetry> {
        if audience.adopted || audience.announced.contains(&peer) || audience.announced.len() >= LOGGED_ANNOUNCEMENTS {
            return None;
        }
        audience.announced.push(peer);
        let rank = audience.announced.len() as u64;
        let slot_latency_ms = audience.slot_onset.map(|onset| latency_since_onset(onset, at));
        Some(HeaderTelemetry::Announced { hash, peer, rank, slot_latency_ms })
    }

    fn note_delivery(
        &mut self,
        peer: Peer,
        hash: HeaderHash,
        height: BlockHeight,
        at: Instant,
    ) -> Option<HeaderTelemetry> {
        let audience = self.audience_mut(hash, height);
        if audience.delivered.contains(&peer) {
            return None;
        }
        audience.delivered.push(peer);
        let rank = audience.delivered.len() as u64;
        let slot_latency_ms = audience.slot_onset.map(|onset| latency_since_onset(onset, at));
        let fetch_latency_ms = audience.asked_at.get(&peer).map(|asked| since_ms(*asked, at));
        Some(HeaderTelemetry::Received { hash, peer, rank, slot_latency_ms, fetch_latency_ms })
    }

    fn audience_mut(&mut self, hash: HeaderHash, height: BlockHeight) -> &mut HeaderAudience {
        let audience = self.audience.entry(hash).or_insert_with(|| HeaderAudience {
            height,
            announced: Vec::new(),
            delivered: Vec::new(),
            adopted: false,
            slot_onset: None,
            asked_at: BTreeMap::new(),
        });
        // A fetch ask may have opened the entry before the header's height was known.
        if audience.height == BlockHeight::from(0) && height != BlockHeight::from(0) {
            audience.height = height;
        }
        audience
    }
}

/// Milliseconds from the slot onset to `at`.
fn latency_since_onset(onset: Duration, at: Instant) -> u64 {
    at.duration_since_global_epoch().saturating_sub(onset).as_millis() as u64
}

/// Milliseconds from `started` to `now`.
fn since_ms(started: Instant, now: Instant) -> u64 {
    now.saturating_since(started).as_millis() as u64
}

impl HeaderPerformance {
    /// Close a lifecycle if present; return zero or one telemetry payload.
    ///
    /// When `syncing` is true, `slot_start_to_header_micros` is omitted from the payload.
    fn close_lifecycle(
        &mut self,
        hash: &HeaderHash,
        outcome: HeaderLifecycleOutcome,
        now: Instant,
        syncing: bool,
    ) -> Vec<HeaderTelemetry> {
        let Some(lifecycle) = self.lifecycles.remove(hash) else {
            return Vec::new();
        };

        let slot_start_to_header_micros = (!syncing).then_some(lifecycle.slot_start_to_header_micros);
        let block_fetch_wait_micros =
            lifecycle.requested_at.map(|requested_at| duration_micros(lifecycle.received_at, requested_at));
        let block_fetch_micros = lifecycle
            .requested_at
            .zip(lifecycle.downloaded_at)
            .map(|(requested_at, downloaded_at)| duration_micros(requested_at, downloaded_at));
        let forward_micros = duration_micros(lifecycle.received_at, now);

        vec![HeaderTelemetry::Lifecycle {
            hash: Some(*hash),
            peer: Some(lifecycle.peer),
            outcome,
            error: None,
            slot_start_to_header_micros,
            block_fetch_wait_micros,
            block_fetch_micros,
            forward_micros: Some(forward_micros),
            adopt_micros: None,
        }]
    }

    fn close_fork(&mut self, hash: &HeaderHash, outcome: ForkSwitchOutcome, now: Instant) -> Vec<HeaderTelemetry> {
        if self.fork_switch.as_ref().is_some_and(|fork| &fork.hash == hash)
            && let Some(fork) = self.fork_switch.take()
        {
            return vec![fork_telemetry(&fork.hash, outcome, now, fork.started_at)];
        }
        Vec::new()
    }
}

/// Number of microseconds elapsed between `started` and `now` (0 if `now` precedes `started`).
fn duration_micros(started: Instant, now: Instant) -> u64 {
    now.saturating_since(started).as_micros() as u64
}

fn fork_telemetry(hash: &HeaderHash, outcome: ForkSwitchOutcome, now: Instant, started_at: Instant) -> HeaderTelemetry {
    HeaderTelemetry::ForkSwitch { hash: *hash, outcome, duration_micros: duration_micros(started_at, now) }
}

fn record_metric(meter: Option<&Meter>, metric: ConsensusMetrics) {
    if let Some(meter) = meter {
        metric.record_to_meter(meter);
    }
}
