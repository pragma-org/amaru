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

use crate::{consensus::store_effects::StoreHeaderEffect};

use super::DecodedChainSyncEvent;
use pure_stage::{Effects, StageRef};
use tracing::{Level, instrument};
use amaru_kernel::span::adopt_current_span;

#[instrument(
    level = Level::TRACE,
    skip_all,
    name = "stage.store_header",
)]
pub async fn stage(
    downstream: StageRef<DecodedChainSyncEvent>,
    msg: DecodedChainSyncEvent,
    eff: Effects<DecodedChainSyncEvent>,
) -> StageRef<DecodedChainSyncEvent> {
    adopt_current_span(&msg);
    match &msg {
        DecodedChainSyncEvent::RollForward {
            peer,
            point,
            header,
            ..
        } => {
            if let Err(error) = eff
                .external(StoreHeaderEffect::new(header.clone(), point.clone()))
                .await
            {
                tracing::error!(%error, %point, %peer, "Failed to store header");
                // FIXME what should be the consequence of this?
                return eff.terminate().await;
            };
            eff.send(&downstream, msg).await
        }
        DecodedChainSyncEvent::Rollback { .. } => eff.send(&downstream, msg).await,
        DecodedChainSyncEvent::CaughtUp { .. } => eff.send(&downstream, msg).await,
    }
    downstream
}
