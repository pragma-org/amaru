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

//! External-effect constructors and implementations for [`Performance`] events.
//!
//! Stages use factory methods on [`Performance`] and pass the result to `eff.external(...)`.
//! Each effect is enqueued as a [`crate::performance::ops::PerformanceOp`] on the worker thread.
//!
//! Header/fork telemetry is produced on the worker as pure [`HeaderTelemetry`] values and
//! **emitted here** on the pure-stage effect executor. That keeps OpenTelemetry export (which may
//! drop or lag under resource/connectivity pressure) off the performance worker.

mod chain;
mod fetch;
mod header;
mod lifecycle;
mod selection;
mod sharing;

use std::sync::Arc;

use amaru_protocols::metrics_effects::ResourceMeter;
use amaru_pure_stage::Resources;
pub use chain::*;
pub use fetch::*;
pub use header::*;
pub use lifecycle::*;
pub use selection::*;
pub use sharing::*;
use tokio::sync::oneshot;

use super::{HeaderTelemetry, Performance, ResourcePerformance, ops::PerformanceOp};

fn require_perf(resources: &Resources) -> ResourcePerformance {
    #[expect(clippy::expect_used)]
    resources.get::<ResourcePerformance>().expect("Performance effect requires ResourcePerformance").clone()
}

fn optional_meter(resources: &Resources) -> Option<Arc<amaru_metrics::Meter>> {
    resources.get::<ResourceMeter>().ok().map(|m| m.clone())
}

/// Fire-and-forget: enqueue and return immediately (does not wait for the worker).
fn enqueue(perf: &Performance, op: PerformanceOp) {
    perf.submit(op);
}

/// Enqueue a query and await the oneshot reply without blocking a multi-thread Tokio worker.
async fn enqueue_query<T: Send + 'static>(
    perf: &Performance,
    make: impl FnOnce(oneshot::Sender<T>) -> PerformanceOp,
) -> T {
    let (reply_tx, reply_rx) = oneshot::channel();
    perf.submit(make(reply_tx));
    #[expect(clippy::expect_used)]
    {
        reply_rx.await.expect("performance worker dropped reply")
    }
}

/// Await worker telemetry, then emit on this (effect-executor) path.
async fn enqueue_and_emit_telemetry(
    perf: &Performance,
    resources: Resources,
    make: impl FnOnce(oneshot::Sender<Vec<HeaderTelemetry>>) -> PerformanceOp,
) {
    let events = enqueue_query(perf, make).await;
    let meter = optional_meter(&resources);
    let live = crate::consensus_mode::is_live(&resources);
    HeaderTelemetry::emit_all(&events, meter.as_deref(), live);
}
