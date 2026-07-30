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

use std::collections::BTreeMap;

use amaru_kernel::{HeaderHash, Tip};
use amaru_metrics::{Meter, MetricRecorder, consensus::ConsensusMetrics};
use amaru_observability::debug;
use amaru_pure_stage::Instant;
use serde::{Deserialize, Serialize};

/// Tracks the processing of headers to emit a single `perf.header.lifecycle` event per header when
/// its block reaches a terminal state (adopted, invalidated or abandoned). The event covers the four
/// network-health processing points of a header's lifecycle and carries the intervals between them:
///
/// - `block_fetch_wait_micros`: from the header's reception to the request of its block.
/// - `block_fetch_micros`: from the request of the block to its reception.
/// - `forward_micros`: from the header's reception to the adoption of its block.
///
/// Fork switches are tracked independently and emitted as `perf.fork.switch`.
///
/// OTel events and metrics are emitted from this type when terminal outcomes are recorded
/// (optional `Meter` from pure-stage resources).
/// Owned by the performance worker thread.
#[derive(Debug, Default)]
pub struct HeaderPerformance {
    /// The lifecycle timestamps of each header whose block has not yet reached a terminal state.
    lifecycles: BTreeMap<HeaderHash, HeaderLifecycle>,
    /// An in-progress fork switch, if any.
    fork_switch: Option<ForkSwitch>,
}

/// The processing timestamps accumulated for a header until its block reaches a terminal state.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct HeaderLifecycle {
    /// Time when the header was first received from an upstream peer.
    received_at: Instant,
    /// Time when its block was first requested, if it was requested.
    requested_at: Option<Instant>,
    /// Time when its block was first received, if it was received.
    downloaded_at: Option<Instant>,
}

impl HeaderLifecycle {
    fn new(received_at: Instant) -> Self {
        Self { received_at, requested_at: None, downloaded_at: None }
    }
}

/// The in-progress switch to a new fork, with the hash of its expected new best tip and the
/// time it was detected.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AbandonedBlock => "abandoned",
            Self::InvalidBlock => "invalid",
            Self::ValidBlock => "valid",
            Self::DuplicateHeader => "duplicate header",
            Self::InvalidHeader => "invalid header",
            Self::UndecodableHeader => "undecodable header",
            Self::StoreHeaderError => "store header error",
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
    fn as_str(self) -> &'static str {
        match self {
            Self::AbandonedBlock => "abandoned",
            Self::InvalidBlock => "invalid",
            Self::ValidBlock => "valid",
            Self::Superseded => "superseded fork",
        }
    }
}

impl HeaderPerformance {
    pub fn new() -> Self {
        Self::default()
    }

    /// A header has been accepted from upstream: start tracking its lifecycle from `received_at`.
    /// Subsequent announcements of the same header do not move `received_at` (first wins).
    pub fn apply_header_received(&mut self, tip: Tip, received_at: Instant) {
        self.lifecycles.entry(tip.hash()).or_insert_with(|| HeaderLifecycle::new(received_at));
    }

    /// The fetch stage requested the blocks for these headers: record their request time.
    pub fn apply_blocks_requested(&mut self, hashes: &[HeaderHash], requested_at: Instant) {
        for hash in hashes {
            if let Some(lifecycle) = self.lifecycles.get_mut(hash) {
                lifecycle.requested_at.get_or_insert(requested_at);
            }
        }
    }

    /// The fetch stage received a block for this header: record the reception time.
    pub fn apply_block_downloaded(&mut self, hash: &HeaderHash, downloaded_at: Instant) {
        if let Some(lifecycle) = self.lifecycles.get_mut(hash) {
            lifecycle.downloaded_at.get_or_insert(downloaded_at);
        }
    }

    /// Header rejected on reception (duplicate, undecodable, invalid, store error, …).
    /// Emits a lifecycle event/metric with no duration fields; does not require a tracked lifecycle.
    pub fn apply_header_rejected(&self, outcome: HeaderLifecycleOutcome, meter: Option<&Meter>) {
        emit_rejected(outcome, meter);
    }

    /// A received header that is abandoned because its block depends on an invalid block.
    pub fn apply_header_abandoned(&mut self, hash: &HeaderHash, now: Instant, meter: Option<&Meter>) {
        self.emit_lifecycle(hash, HeaderLifecycleOutcome::AbandonedBlock, now, meter);
    }

    /// A fork has been detected: start tracking the time it takes to switch to it. If a previous
    /// fork switch is still in progress, record it as superseded.
    pub fn apply_fork_started(&mut self, tip: Tip, started_at: Instant, meter: Option<&Meter>) {
        if let Some(previous) = self.fork_switch.take() {
            emit_fork_switch(&previous.hash, ForkSwitchOutcome::Superseded, started_at, previous.started_at, meter);
        }
        self.fork_switch = Some(ForkSwitch { hash: tip.hash(), started_at });
    }

    /// The block for a header has been validated and adopted: emit its lifecycle event and the
    /// fork-switch event if it was waiting on this header.
    pub fn apply_block_valid(&mut self, hash: &HeaderHash, now: Instant, meter: Option<&Meter>) {
        self.emit_lifecycle(hash, HeaderLifecycleOutcome::ValidBlock, now, meter);
        self.close_fork(hash, ForkSwitchOutcome::ValidBlock, now, meter);
    }

    /// A header has been pruned after a block validation.
    ///
    /// `invalid == true` means that the block that was found invalid; otherwise this header/block
    /// has been abandoned because a better chain is available.
    pub fn apply_block_pruned(&mut self, hash: &HeaderHash, invalid: bool, now: Instant, meter: Option<&Meter>) {
        let (header_outcome, fork_outcome) = if invalid {
            (HeaderLifecycleOutcome::InvalidBlock, ForkSwitchOutcome::InvalidBlock)
        } else {
            (HeaderLifecycleOutcome::AbandonedBlock, ForkSwitchOutcome::AbandonedBlock)
        };
        self.emit_lifecycle(hash, header_outcome, now, meter);
        self.close_fork(hash, fork_outcome, now, meter);
    }

    /// Number of headers currently tracked (tests / diagnostics).
    pub fn lifecycle_count(&self) -> usize {
        self.lifecycles.len()
    }

    /// Whether a fork switch is in progress for `hash` (tests / diagnostics).
    pub fn has_fork_switch(&self, hash: &HeaderHash) -> bool {
        self.fork_switch.as_ref().is_some_and(|f| &f.hash == hash)
    }
}

impl HeaderPerformance {
    /// Emit a `perf.header.lifecycle` event for a header reaching a terminal state.
    /// Record the corresponding metric, and drop its tracking record.
    fn emit_lifecycle(
        &mut self,
        hash: &HeaderHash,
        outcome: HeaderLifecycleOutcome,
        now: Instant,
        meter: Option<&Meter>,
    ) {
        let Some(lifecycle) = self.lifecycles.remove(hash) else {
            return;
        };

        let block_fetch_wait_micros =
            lifecycle.requested_at.map(|requested_at| duration_micros(lifecycle.received_at, requested_at));
        let block_fetch_micros = lifecycle
            .requested_at
            .zip(lifecycle.downloaded_at)
            .map(|(requested_at, downloaded_at)| duration_micros(requested_at, downloaded_at));
        let forward_micros = duration_micros(lifecycle.received_at, now);

        debug!(
            consensus::perf::header::LIFECYCLE,
            header_hash = hash,
            outcome = outcome.as_str(),
            block_fetch_wait_micros = @block_fetch_wait_micros,
            block_fetch_micros = @block_fetch_micros,
            forward_micros = @forward_micros
        );

        record_metric(
            meter,
            ConsensusMetrics::HeaderLifecycle {
                outcome: outcome.as_str().to_string(),
                block_fetch_wait_micros,
                block_fetch_micros,
                forward_micros: Some(forward_micros),
            },
        );
    }

    fn close_fork(&mut self, hash: &HeaderHash, outcome: ForkSwitchOutcome, now: Instant, meter: Option<&Meter>) {
        if self.fork_switch.as_ref().is_some_and(|fork| &fork.hash == hash)
            && let Some(fork) = self.fork_switch.take()
        {
            emit_fork_switch(&fork.hash, outcome, now, fork.started_at, meter);
        }
    }
}

/// Number of microseconds elapsed between `started` and `now` (0 if `now` precedes `started`).
fn duration_micros(started: Instant, now: Instant) -> u64 {
    now.saturating_since(started).as_micros() as u64
}

fn emit_rejected(outcome: HeaderLifecycleOutcome, meter: Option<&Meter>) {
    debug!(consensus::perf::header::LIFECYCLE, outcome = outcome.as_str());
    record_metric(
        meter,
        ConsensusMetrics::HeaderLifecycle {
            outcome: outcome.as_str().to_string(),
            block_fetch_wait_micros: None,
            block_fetch_micros: None,
            forward_micros: None,
        },
    );
}

/// Emit a `perf.fork.switch` event for a fork switch that started at `started_at` and ended now,
/// and record the matching metric when a meter is available.
fn emit_fork_switch(
    hash: &HeaderHash,
    outcome: ForkSwitchOutcome,
    now: Instant,
    started_at: Instant,
    meter: Option<&Meter>,
) {
    let duration = duration_micros(started_at, now);
    debug!(
        consensus::perf::fork::SWITCH,
        header_hash = hash,
        outcome = outcome.as_str(),
        duration_micros = @duration
    );
    record_metric(
        meter,
        ConsensusMetrics::ForkSwitch { outcome: outcome.as_str().to_string(), duration_micros: duration },
    );
}

fn record_metric(meter: Option<&Meter>, metric: ConsensusMetrics) {
    if let Some(meter) = meter {
        metric.record_to_meter(meter);
    }
}
