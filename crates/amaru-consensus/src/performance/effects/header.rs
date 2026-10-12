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

use amaru_kernel::{BlockHeight, HeaderHash, Point};
use amaru_pure_stage::{BoxFuture, ExternalEffectAPI, Instant, Resources, SendData};

use super::{enqueue, enqueue_and_emit_telemetry, enqueue_query, optional_meter, require_perf};
use crate::performance::{
    HeaderLifecycleOutcome, HeaderPerformance, Performance,
    ops::{HeaderOp, PaceOp, PerformanceOp},
};

impl Performance {
    pub fn prune_below(min_height: BlockHeight, now: Instant) -> PruneBelowEffect {
        PruneBelowEffect { min_height, now }
    }

    pub fn record_header_rejected(outcome: HeaderLifecycleOutcome) -> RecordHeaderRejectedEffect {
        RecordHeaderRejectedEffect { outcome }
    }

    pub fn record_header_abandoned(hash: HeaderHash, now: Instant) -> RecordHeaderAbandonedEffect {
        RecordHeaderAbandonedEffect { hash, now }
    }

    pub fn record_fork_started(tip: Point, started_at: Instant) -> RecordForkStartedEffect {
        RecordForkStartedEffect { tip, started_at }
    }

    pub fn record_block_valid(hash: HeaderHash, now: Instant, syncing: bool) -> RecordBlockValidEffect {
        RecordBlockValidEffect { hash, now, syncing }
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
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PruneBelowEffect {
    pub(crate) min_height: BlockHeight,
    pub(crate) now: Instant,
}

impl ExternalEffectAPI for PruneBelowEffect {
    type Response = ();

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
}

impl ExternalEffectAPI for RecordHeaderRejectedEffect {
    type Response = ();

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            // NOTE: No worker state; emit directly on the effect path (never on the performance thread).
            let meter = optional_meter(&resources);
            HeaderPerformance::apply_header_rejected(self.outcome).emit(meter.as_deref(), false);
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

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let perf = require_perf(&resources);
        let resources = resources.clone();
        self.wrap(|this| async move {
            enqueue_and_emit_telemetry(&perf, resources, |reply| {
                PerformanceOp::Header(HeaderOp::RecordForkStarted { effect: this, reply })
            })
            .await
        })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RecordBlockValidEffect {
    pub(crate) hash: HeaderHash,
    pub(crate) now: Instant,
    /// When true, omit `slot_start_to_header_micros` from the emitted lifecycle metric.
    pub(crate) syncing: bool,
}

impl ExternalEffectAPI for RecordBlockValidEffect {
    type Response = ();

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

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let perf = require_perf(&resources);
        self.wrap(|this| async move {
            enqueue_query(&perf, |reply| PerformanceOp::Pace(PaceOp::SyncAdoptionPace { effect: this, reply })).await
        })
    }
}
