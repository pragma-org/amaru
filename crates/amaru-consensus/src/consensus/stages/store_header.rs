// Copyright 2025 PRAGMA
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

use crate::consensus::effects::{BaseOps, ConsensusOps};
use crate::consensus::events::NewHeader;
use crate::consensus::span::adopt_current_span;
use pure_stage::StageRef;
use tracing::{Level, instrument};

#[instrument(
    level = Level::TRACE,
    skip_all,
    name = "stage.store_header",
)]
pub async fn stage(
    downstream: StageRef<NewHeader>,
    msg: NewHeader,
    eff: impl ConsensusOps,
) -> StageRef<NewHeader> {
    adopt_current_span(&msg);
    let result = eff.store().store_header(msg.header());
    if let Err(error) = result {
        let peer = msg.peer();
        let point = msg.point();
        tracing::error!(%error, %peer, "Failed to store header at {point}");
        // FIXME what should be the consequence of this?
        eff.base().terminate().await;
    };
    eff.base().send(&downstream, msg).await;
    downstream
}
