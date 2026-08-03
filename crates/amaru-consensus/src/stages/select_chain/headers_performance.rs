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

use std::{collections::BTreeMap, time::Duration};

use amaru_kernel::{EraHistory, HeaderHash, Peer, Tip};
use amaru_metrics::consensus::ConsensusMetrics;
use amaru_observability::debug;
use amaru_protocols::metrics_effects::{Metrics, MetricsOps};
use amaru_pure_stage::{Effects, Instant, Void};
use serde::{Deserialize, Serialize};

/// Tracks the processing of headers to emit a single `perf.header.lifecycle` event per header when
/// its block reaches a terminal state (adopted, invalidated or abandoned). The event covers the
/// virtual slot start followed by the four network-health processing points of a header's lifecycle
/// and carries the intervals between them:
///
/// - `slot_start_to_header_micros`: from the virtual beginning of the slot to the header's
///   reception.
/// - `block_fetch_wait_micros`: from the header's reception to the request of its block.
/// - `block_fetch_micros`: from the request of the block to its reception.
/// - `forward_micros`: from the header's reception to the adoption of its block.
///
/// Fork switches are tracked independently and emitted as `perf.fork.switch`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HeadersPerformance {
    era_history: EraHistory,
    /// The lifecycle timestamps of each header whose block has not yet reached a terminal state.
    lifecycles: BTreeMap<HeaderHash, HeaderLifecycle>,
    /// An in-progress fork switch, if any.
    fork_switch: Option<ForkSwitch>,
}

/// The processing timestamps accumulated for a header until its block reaches a terminal state.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct HeaderLifecycle {
    /// Peer from which the header was first received.
    peer: Peer,
    /// Virtual start of the header's slot, relative to the network global epoch.
    slot_start: Duration,
    /// Time when the header was first received from an upstream peer.
    received_at: Instant,
    /// Time when its block was first requested, if it was requested.
    requested_at: Option<Instant>,
    /// Time when its block was first received, if it was received.
    downloaded_at: Option<Instant>,
}

impl HeaderLifecycle {
    fn new(peer: Peer, slot_start: Duration, received_at: Instant) -> Self {
        Self { peer, slot_start, received_at, requested_at: None, downloaded_at: None }
    }
}

/// The in-progress switch to a new fork, with the hash of its expected new best tip and the
/// time it was detected.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ForkSwitch {
    hash: HeaderHash,
    started_at: Instant,
}

impl HeadersPerformance {
    pub fn new(era_history: EraHistory) -> Self {
        Self { era_history, ..Self::default() }
    }

    /// A header has been accepted from upstream: start tracking its lifecycle from `received_at`.
    pub fn header_received(&mut self, peer: Peer, tip: Tip, received_at: Instant) {
        let slot_start = self.era_history.slot_to_relative_time_unchecked_horizon(tip.slot()).unwrap_or_default();
        self.lifecycles.entry(tip.hash()).or_insert_with(|| HeaderLifecycle::new(peer, slot_start, received_at));
    }

    /// The fetch_blocks stage requested the blocks for these headers: record their request time.
    pub fn blocks_requested(&mut self, hashes: &[HeaderHash], requested_at: Instant) {
        for hash in hashes {
            if let Some(lifecycle) = self.lifecycles.get_mut(hash) {
                lifecycle.requested_at.get_or_insert(requested_at);
            }
        }
    }

    /// The fetch_blocks stage received a block for this header: record the reception time.
    pub fn block_downloaded(&mut self, hash: &HeaderHash, downloaded_at: Instant) {
        if let Some(lifecycle) = self.lifecycles.get_mut(hash) {
            lifecycle.downloaded_at.get_or_insert(downloaded_at);
        }
    }

    /// A received header that is abandoned because its block depends on an invalid block.
    pub async fn header_abandoned(&mut self, eff: &Effects<Void>, hash: &HeaderHash, now: Instant) {
        self.emit_lifecycle(eff, hash, PerfHeaderForwardOutcome::AbandonedBlock, now).await;
    }

    /// A fork has been detected: start tracking the time it takes to switch to it. If a previous
    /// fork switch is still in progress, record it as superseded.
    pub async fn fork_started(&mut self, eff: &Effects<Void>, tip: Tip, started_at: Instant) {
        if let Some(previous) = self.fork_switch.take() {
            let ops = Metrics::new(eff);
            ops.record(
                emit_fork_switch(&previous.hash, PerfForkSwitchOutcome::Superseded, started_at, previous.started_at)
                    .into(),
            )
            .await;
        }
        self.fork_switch = Some(ForkSwitch { hash: tip.hash(), started_at });
    }

    /// The block for a header has been validated and adopted: emit its lifecycle event and the
    /// fork-switch event if it was waiting on this header.
    pub async fn block_valid(&mut self, eff: &Effects<Void>, hash: &HeaderHash, now: Instant) {
        self.emit_lifecycle(eff, hash, PerfHeaderForwardOutcome::ValidBlock, now).await;
        self.close_fork(eff, hash, PerfForkSwitchOutcome::ValidBlock, now).await;
    }

    /// A header has been pruned after a block validation.
    /// `invalid == true` means that the block that was found invalid
    /// Otherwise this header/block has been abandoned because a better chain is available.
    pub async fn block_pruned(&mut self, eff: &Effects<Void>, hash: &HeaderHash, invalid: bool, now: Instant) {
        let (header_outcome, fork_outcome) = if invalid {
            (PerfHeaderForwardOutcome::InvalidBlock, PerfForkSwitchOutcome::InvalidBlock)
        } else {
            (PerfHeaderForwardOutcome::AbandonedBlock, PerfForkSwitchOutcome::AbandonedBlock)
        };
        self.emit_lifecycle(eff, hash, header_outcome, now).await;
        self.close_fork(eff, hash, fork_outcome, now).await;
    }

    /// Emit a `perf.header.lifecycle` event for a header reaching a terminal state.
    /// Record the corresponding metric, and drop its tracking record.
    async fn emit_lifecycle(
        &mut self,
        eff: &Effects<Void>,
        hash: &HeaderHash,
        outcome: PerfHeaderForwardOutcome,
        now: Instant,
    ) {
        let Some(lifecycle) = self.lifecycles.remove(hash) else {
            return;
        };

        let slot_start_to_header_micros =
            duration_micros_since(lifecycle.slot_start, lifecycle.received_at.duration_since_global_epoch());
        let block_fetch_wait_micros =
            lifecycle.requested_at.map(|requested_at| duration_micros(lifecycle.received_at, requested_at));
        let block_fetch_micros = lifecycle
            .requested_at
            .zip(lifecycle.downloaded_at)
            .map(|(requested_at, downloaded_at)| duration_micros(requested_at, downloaded_at));
        let forward_micros = duration_micros(lifecycle.received_at, now);

        debug!(
            consensus::perf::header::LIFECYCLE,
            peer = lifecycle.peer.clone(),
            header_hash = hash,
            outcome = outcome.as_str(),
            slot_start_to_header_micros = @slot_start_to_header_micros,
            block_fetch_wait_micros = @block_fetch_wait_micros,
            block_fetch_micros = @block_fetch_micros,
            forward_micros = @forward_micros
        );

        let ops = Metrics::new(eff);
        ops.record(
            ConsensusMetrics::HeaderLifecycle {
                outcome: outcome.as_str().to_string(),
                slot_start_to_header_micros: Some(slot_start_to_header_micros),
                block_fetch_wait_micros,
                block_fetch_micros,
                forward_micros: Some(forward_micros),
            }
            .into(),
        )
        .await;
    }

    async fn close_fork(
        &mut self,
        eff: &Effects<Void>,
        hash: &HeaderHash,
        outcome: PerfForkSwitchOutcome,
        now: Instant,
    ) {
        if self.fork_switch.as_ref().is_some_and(|fork| &fork.hash == hash)
            && let Some(fork) = self.fork_switch.take()
        {
            let ops = Metrics::new(eff);
            ops.record(emit_fork_switch(&fork.hash, outcome, now, fork.started_at).into()).await
        }
    }
}

impl PartialEq for HeadersPerformance {
    fn eq(&self, other: &Self) -> bool {
        // The recorded times are wall-clock values used only to compute durations on close, so
        // equality compares only which headers are tracked, keeping it independent of timing.
        self.lifecycles.keys().eq(other.lifecycles.keys())
            && self.fork_switch.as_ref().map(|fork| fork.hash) == other.fork_switch.as_ref().map(|fork| fork.hash)
    }
}

/// Number of microseconds elapsed between `started` and `now` (0 if `now` precedes `started`).
fn duration_micros(started: Instant, now: Instant) -> u64 {
    now.saturating_since(started).as_micros() as u64
}

fn duration_micros_since(started: Duration, now: Duration) -> u64 {
    now.saturating_sub(started).as_micros() as u64
}

/// Emit a `perf.fork.switch` event for a fork switch that started at `started_at` and ended now,
/// and return the matching metric.
fn emit_fork_switch(
    hash: &HeaderHash,
    outcome: PerfForkSwitchOutcome,
    now: Instant,
    started_at: Instant,
) -> ConsensusMetrics {
    let duration = duration_micros(started_at, now);
    debug!(consensus::perf::fork::SWITCH, header_hash = hash, outcome = outcome.as_str(), duration_micros = @duration);
    ConsensusMetrics::ForkSwitch { outcome: outcome.as_str().to_string(), duration_micros: duration }
}

/// Local enum modelling the outcome of a header's lifecycle
#[derive(Debug, Clone, Copy)]
pub enum PerfHeaderForwardOutcome {
    /// We stopped trying to get the validity of a block (it might still have been downloaded in the
    /// meantime).
    AbandonedBlock,
    /// The block was retrieved but validation failed.
    InvalidBlock,
    /// The block was retrieved and validated.
    ValidBlock,
    /// The header for a given block has already been received
    DuplicateHeader,
    /// The header cannot be decoded
    UndecodableHeader,
    /// The header is invalid
    InvalidHeader,
    /// The header for that block could not be stored
    StoreHeaderError,
}

impl PerfHeaderForwardOutcome {
    pub(crate) fn as_str(self) -> &'static str {
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

/// Local enum modelling the outcome of the perf.fork.switch event
#[derive(Debug, Clone, Copy)]
pub enum PerfForkSwitchOutcome {
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

impl PerfForkSwitchOutcome {
    fn as_str(self) -> &'static str {
        match self {
            Self::AbandonedBlock => "abandoned",
            Self::InvalidBlock => "invalid",
            Self::ValidBlock => "valid",
            Self::Superseded => "superseded fork",
        }
    }
}

#[cfg(test)]
impl HeadersPerformance {
    /// Track the given hashes as lifecycles started at time zero (test helper).
    pub(crate) fn with_lifecycles(mut self, hashes: impl IntoIterator<Item = HeaderHash>) -> Self {
        for hash in hashes {
            self.lifecycles.insert(
                hash,
                HeaderLifecycle::new(
                    Peer::new("upstream"),
                    std::time::Duration::ZERO,
                    Instant::at_offset(std::time::Duration::ZERO, std::time::Duration::ZERO),
                ),
            );
        }
        self
    }

    /// Track an in-progress fork switch for the given header hash (test helper).
    pub(crate) fn with_fork_switch(mut self, hash: HeaderHash) -> Self {
        self.fork_switch = Some(ForkSwitch {
            hash,
            started_at: Instant::at_offset(std::time::Duration::ZERO, std::time::Duration::ZERO),
        });
        self
    }

    pub(crate) fn with_fork_switch_at(mut self, hash: HeaderHash, started_at: std::time::Duration) -> Self {
        self.fork_switch = Some(ForkSwitch { hash, started_at: Instant::at_offset(started_at, started_at) });
        self
    }
}
