// Copyright 2024 PRAGMA
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

use crate::consensus::{effects::ChainSyncEffect, span::HasSpan};
use amaru_kernel::consensus_events::{ChainSyncEvent, Tracked};
use pure_stage::{Effects, StageRef};
use tracing::Instrument;
use tracing_opentelemetry::OpenTelemetrySpanExt;

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct NextSync;

pub async fn stage(
    downstream: StageRef<Tracked<ChainSyncEvent>>,
    _msg: NextSync,
    eff: Effects<NextSync>,
) -> StageRef<Tracked<ChainSyncEvent>> {
    let span = tracing::trace_span!("stage.pull_wait");
    let mut msg = eff.external(ChainSyncEffect).instrument(span).await;

    let span = tracing::trace_span!("stage.pull");
    span.set_parent(msg.span().context()).ok();
    let entered = span.enter();

    match &mut msg {
        Tracked::Wrapped(event) => event.set_span(span.clone()),
        Tracked::CaughtUp { span: s, .. } => *s = span.clone(),
    };

    drop(entered);
    async {
        eff.send(&downstream, msg).await;
        eff.send(eff.me_ref(), NextSync).await;
    }
    .instrument(span)
    .await;
    downstream
}
