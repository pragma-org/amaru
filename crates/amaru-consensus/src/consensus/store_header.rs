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

use crate::{consensus::store_effects::StoreHeaderEffect, span::adopt_current_span};
use async_trait::async_trait;

use super::DecodedChainSyncEvent;
use pure_stage::{Effects, Stage, StageRef};
use tracing::{Level, instrument};

#[derive(Clone)]
pub struct StoreHeader {
    downstream: StageRef<DecodedChainSyncEvent>,
}

impl StoreHeader {
    pub fn new(downstream: impl AsRef<StageRef<DecodedChainSyncEvent>>) -> Self {
        Self {
            downstream: downstream.as_ref().clone(),
        }
    }
}

#[async_trait]
impl Stage<DecodedChainSyncEvent, ()> for StoreHeader {
    fn initial_state(&self) {}

    #[instrument(level = Level::TRACE, skip_all, name = "stage.store_header")]
    async fn run(
        &self,
        _state: (),
        msg: DecodedChainSyncEvent,
        eff: Effects<DecodedChainSyncEvent>,
    ) -> () {
        adopt_current_span(&msg);
        if let DecodedChainSyncEvent::RollForward {
            ref peer,
            ref point,
            ref header,
            ..
        } = msg
            && let Err(error) = eff
                .external(StoreHeaderEffect::new(header.clone(), point.clone()))
                .await
        {
            tracing::error!(%error, %point, %peer, "Failed to store header");
            // FIXME what should be the consequence of this?
            return eff.terminate().await;
        }
        eff.send(&self.downstream, msg).await
    }
}
