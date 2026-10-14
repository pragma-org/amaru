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

//! Header lifecycle, fork, prune, and sync-adoption pace.

#![expect(clippy::unit_arg)]

use amaru_kernel::{BlockHeight, HeaderHash, Peer, Point};
use amaru_observability::{
    TraceContext,
    amaru::{consensus::roll_forward::PROCESS, network::perf::header::FORWARD},
};
use amaru_pure_stage::{BoxFuture, DurationDist, ExternalEffectAPI, Instant, Resources, SendData};

use super::{enqueue, enqueue_and_emit_telemetry, enqueue_query, require_perf};
use crate::{
    effect_timings,
    performance::{
        HeaderLifecycleOutcome, HeaderTelemetry, Performance,
        ops::{HeaderOp, PaceOp, PerformanceOp},
    },
};

impl Performance {
    pub fn prune_below(min_height: BlockHeight, now: Instant) -> PruneBelowEffect {
        PruneBelowEffect { min_height, now }
    }

    pub fn record_header_rejected(outcome: HeaderLifecycleOutcome) -> RecordHeaderRejectedEffect {
        Self::record_header_rejected_with(outcome, None, None, None)
    }

    pub fn record_header_rejected_with(
        outcome: HeaderLifecycleOutcome,
        peer: Option<Peer>,
        hash: Option<HeaderHash>,
        error: Option<String>,
    ) -> RecordHeaderRejectedEffect {
        RecordHeaderRejectedEffect { outcome, peer, hash, error }
    }

    /// Open `perf.header.forward`. `parent` is the roll-forward span; `None` opens a root span.
    pub fn open_header_forward(hash: HeaderHash, parent: Option<TraceContext<PROCESS>>) -> OpenHeaderForwardEffect {
        OpenHeaderForwardEffect { hash, parent }
    }

    pub fn open_block_fetch_wait(hash: HeaderHash, parent: TraceContext<FORWARD>) -> OpenBlockFetchWaitEffect {
        OpenBlockFetchWaitEffect { hash, parent }
    }

    pub fn close_block_fetch_wait(hash: HeaderHash) -> CloseBlockFetchWaitEffect {
        CloseBlockFetchWaitEffect { hash }
    }

    pub fn open_block_fetches(hashes: Vec<HeaderHash>) -> OpenBlockFetchesEffect {
        OpenBlockFetchesEffect { hashes }
    }

    pub fn close_block_fetch(hash: HeaderHash) -> CloseBlockFetchEffect {
        CloseBlockFetchEffect { hash }
    }

    pub fn close_header_forward(hash: HeaderHash) -> CloseHeaderForwardEffect {
        CloseHeaderForwardEffect { hash }
    }

    pub fn record_header_abandoned(hash: HeaderHash, now: Instant) -> RecordHeaderAbandonedEffect {
        RecordHeaderAbandonedEffect { hash, now }
    }

    pub fn record_fork_started(tip: Point, started_at: Instant) -> RecordForkStartedEffect {
        RecordForkStartedEffect { tip, started_at }
    }

    pub fn record_block_valid(
        hash: HeaderHash,
        height: BlockHeight,
        now: Instant,
        syncing: bool,
    ) -> RecordBlockValidEffect {
        RecordBlockValidEffect { hash, height, now, syncing }
    }

    pub fn record_block_pruned(
        hash: HeaderHash,
        invalid: bool,
        now: Instant,
        syncing: bool,
    ) -> RecordBlockPrunedEffect {
        RecordBlockPrunedEffect { hash, invalid, now, syncing }
    }

    /// Record one chain adoption. `live` clears the sync pace; sync adoptions update it.
    pub fn record_sync_adoption(at: Instant, live: bool) -> RecordSyncAdoptionEffect {
        RecordSyncAdoptionEffect { at, live }
    }

    /// Whether sync adoptions are still arriving faster than 10 per second and are not overdue.
    pub fn sync_adoption_is_fast(now: Instant) -> SyncAdoptionPaceEffect {
        SyncAdoptionPaceEffect { now }
    }

    pub(crate) fn emit_telemetry(&self, events: &[HeaderTelemetry], resources: &Resources) {
        let meter = super::optional_meter(resources);
        let live = crate::consensus_mode::is_live(resources);
        let mut spans = self.spans.lock();
        HeaderTelemetry::emit_all(events, meter.as_deref(), live, &mut spans);
    }

    pub(crate) fn span_open_forward(
        &self,
        hash: HeaderHash,
        parent: Option<TraceContext<PROCESS>>,
    ) -> TraceContext<FORWARD> {
        self.spans.lock().open_forward(hash, parent)
    }

    pub(crate) fn span_open_fetch_wait(&self, hash: HeaderHash, parent: TraceContext<FORWARD>) {
        self.spans.lock().open_fetch_wait(hash, parent);
    }

    pub(crate) fn span_close_fetch_wait(&self, hash: &HeaderHash) {
        self.spans.lock().close_fetch_wait(hash);
    }

    pub(crate) fn span_open_fetches(&self, hashes: &[HeaderHash]) {
        self.spans.lock().open_fetches(hashes);
    }

    pub(crate) fn span_close_fetch(&self, hash: &HeaderHash) {
        self.spans.lock().close_fetch(hash);
    }

    pub(crate) fn span_close_forward(&self, hash: &HeaderHash) {
        self.spans.lock().close_forward(hash);
    }

    pub(crate) fn span_open_fork(&self, hash: &HeaderHash) {
        self.spans.lock().open_fork(hash);
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PruneBelowEffect {
    pub(crate) min_height: BlockHeight,
    pub(crate) now: Instant,
}

impl ExternalEffectAPI for PruneBelowEffect {
    type Response = ();
    const SIMULATED_DURATION: DurationDist = effect_timings::performance__effects__PruneBelowEffect;

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let perf = require_perf(&resources);
        let resources = resources.clone();
        self.wrap(|this| async move {
            enqueue_and_emit_telemetry(&perf, resources, |reply| {
                PerformanceOp::Header(HeaderOp::PruneBelow { effect: this, reply })
            })
            .await
        })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RecordHeaderRejectedEffect {
    pub(crate) outcome: HeaderLifecycleOutcome,
    pub(crate) peer: Option<Peer>,
    pub(crate) hash: Option<HeaderHash>,
    pub(crate) error: Option<String>,
}

impl ExternalEffectAPI for RecordHeaderRejectedEffect {
    type Response = ();

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let perf = require_perf(&resources);
        self.wrap_sync({
            // NOTE: No worker state; emit directly on the effect path (never on the performance thread).
            let event = HeaderTelemetry::rejected_with(self.outcome, self.peer, self.hash, self.error.clone());
            perf.emit_telemetry(std::slice::from_ref(&event), &resources);
        })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct OpenHeaderForwardEffect {
    pub(crate) hash: HeaderHash,
    pub(crate) parent: Option<TraceContext<PROCESS>>,
}

impl ExternalEffectAPI for OpenHeaderForwardEffect {
    type Response = TraceContext<FORWARD>;

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let perf = require_perf(&resources);
        let hash = self.hash;
        let parent = self.parent.clone();
        self.wrap_sync_f(move || perf.span_open_forward(hash, parent))
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct OpenBlockFetchWaitEffect {
    pub(crate) hash: HeaderHash,
    pub(crate) parent: TraceContext<FORWARD>,
}

impl ExternalEffectAPI for OpenBlockFetchWaitEffect {
    type Response = ();

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let perf = require_perf(&resources);
            perf.span_open_fetch_wait(self.hash, self.parent.clone());
        })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CloseBlockFetchWaitEffect {
    pub(crate) hash: HeaderHash,
}

impl ExternalEffectAPI for CloseBlockFetchWaitEffect {
    type Response = ();

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let perf = require_perf(&resources);
            perf.span_close_fetch_wait(&self.hash);
        })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct OpenBlockFetchesEffect {
    pub(crate) hashes: Vec<HeaderHash>,
}

impl ExternalEffectAPI for OpenBlockFetchesEffect {
    type Response = ();

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let perf = require_perf(&resources);
            perf.span_open_fetches(&self.hashes);
        })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CloseBlockFetchEffect {
    pub(crate) hash: HeaderHash,
}

impl ExternalEffectAPI for CloseBlockFetchEffect {
    type Response = ();

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let perf = require_perf(&resources);
            perf.span_close_fetch(&self.hash);
        })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CloseHeaderForwardEffect {
    pub(crate) hash: HeaderHash,
}

impl ExternalEffectAPI for CloseHeaderForwardEffect {
    type Response = ();

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let perf = require_perf(&resources);
            perf.span_close_forward(&self.hash);
        })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RecordHeaderAbandonedEffect {
    pub(crate) hash: HeaderHash,
    pub(crate) now: Instant,
}

impl ExternalEffectAPI for RecordHeaderAbandonedEffect {
    type Response = ();

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let perf = require_perf(&resources);
        let resources = resources.clone();
        self.wrap(|this| async move {
            enqueue_and_emit_telemetry(&perf, resources, |reply| {
                PerformanceOp::Header(HeaderOp::RecordHeaderAbandoned { effect: this, reply })
            })
            .await
        })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RecordForkStartedEffect {
    pub(crate) tip: Point,
    pub(crate) started_at: Instant,
}

impl ExternalEffectAPI for RecordForkStartedEffect {
    type Response = ();
    const SIMULATED_DURATION: DurationDist = effect_timings::performance__effects__RecordForkStartedEffect;

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let perf = require_perf(&resources);
        let resources = resources.clone();
        self.wrap(|this| async move {
            let hash = this.tip.hash();
            enqueue_and_emit_telemetry(&perf, resources, |reply| {
                PerformanceOp::Header(HeaderOp::RecordForkStarted { effect: this, reply })
            })
            .await;
            perf.span_open_fork(&hash);
        })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RecordBlockValidEffect {
    pub(crate) hash: HeaderHash,
    pub(crate) height: BlockHeight,
    pub(crate) now: Instant,
    /// When true, omit `slot_start_to_header_micros` from the emitted lifecycle metric.
    pub(crate) syncing: bool,
}

impl ExternalEffectAPI for RecordBlockValidEffect {
    type Response = ();
    const SIMULATED_DURATION: DurationDist = effect_timings::performance__effects__RecordBlockValidEffect;

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let perf = require_perf(&resources);
        let resources = resources.clone();
        self.wrap(|this| async move {
            enqueue_and_emit_telemetry(&perf, resources, |reply| {
                PerformanceOp::Header(HeaderOp::RecordBlockValid { effect: this, reply })
            })
            .await
        })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RecordBlockPrunedEffect {
    pub(crate) hash: HeaderHash,
    pub(crate) invalid: bool,
    pub(crate) now: Instant,
    /// When true, omit `slot_start_to_header_micros` from the emitted lifecycle metric.
    pub(crate) syncing: bool,
}

impl ExternalEffectAPI for RecordBlockPrunedEffect {
    type Response = ();

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let perf = require_perf(&resources);
        let resources = resources.clone();
        self.wrap(|this| async move {
            enqueue_and_emit_telemetry(&perf, resources, |reply| {
                PerformanceOp::Header(HeaderOp::RecordBlockPruned { effect: this, reply })
            })
            .await
        })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RecordSyncAdoptionEffect {
    pub(crate) at: Instant,
    pub(crate) live: bool,
}

impl ExternalEffectAPI for RecordSyncAdoptionEffect {
    type Response = ();
    const SIMULATED_DURATION: DurationDist = effect_timings::performance__effects__RecordSyncAdoptionEffect;

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let perf = require_perf(&resources);
            enqueue(&perf, PerformanceOp::Pace(PaceOp::RecordSyncAdoption { effect: self.as_ref().clone() }));
        })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SyncAdoptionPaceEffect {
    pub(crate) now: Instant,
}

impl ExternalEffectAPI for SyncAdoptionPaceEffect {
    type Response = bool;
    const SIMULATED_DURATION: DurationDist = effect_timings::performance__effects__SyncAdoptionPaceEffect;

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let perf = require_perf(&resources);
        self.wrap(|this| async move {
            enqueue_query(&perf, |reply| PerformanceOp::Pace(PaceOp::SyncAdoptionPace { effect: this, reply })).await
        })
    }
}
