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

use crate::consensus::effects::ChainSyncEffect;
use amaru_kernel::consensus_events::{ChainSyncEvent, Tracked};
use pure_stage::{Effects, StageRef};
use tracing::{Level, span};

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct NextSync;

pub async fn stage(
    downstream: StageRef<Tracked<ChainSyncEvent>>,
    _msg: NextSync,
    eff: Effects<NextSync>,
) -> StageRef<Tracked<ChainSyncEvent>> {
    let span = span!(Level::TRACE, "stage.pull");
    let _entered = span.enter();

    let mut msg = eff.external(ChainSyncEffect).await;

    // Set the span on the message so that stage.pull is the start of the trace
    match &mut msg {
        Tracked::Wrapped(event) => event.set_span(span.clone()),
        Tracked::CaughtUp { span: s, .. } => *s = span.clone(),
    };

    eff.send(&downstream, msg).await;
    eff.send(eff.me_ref(), NextSync).await;
    downstream
}
