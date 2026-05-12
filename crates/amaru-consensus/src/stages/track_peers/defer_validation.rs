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

//! Sub-stage used by [`super::stage`]: holds header validations that failed transiently
//! (because the ledger has not yet computed the required stake distribution) and
//! re-injects them into the parent `track_peers` stage on a polling interval.

use std::time::Duration;

use pure_stage::{Effects, StageRef};

use super::{PendingValidation, TrackPeersMsg};

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum DeferValidationMsg {
    Register(PendingValidation),
    Poll,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DeferValidation {
    pub poll_interval_ms: u64,
    pub track_peers: StageRef<TrackPeersMsg>,
    pub pending: Vec<PendingValidation>,
}

impl DeferValidation {
    pub fn new(poll_interval_ms: u64, track_peers: StageRef<TrackPeersMsg>) -> Self {
        Self { poll_interval_ms, track_peers, pending: Vec::new() }
    }
}

pub async fn stage(
    mut state: DeferValidation,
    msg: DeferValidationMsg,
    eff: Effects<DeferValidationMsg>,
) -> DeferValidation {
    use DeferValidationMsg::*;
    match msg {
        Register(p) => {
            state.pending.push(p);
        }
        Poll => {
            // Drain all pending and re-inject. If validation fails again with the same
            // transient condition, `TrackPeers` will re-register the entry.
            for p in std::mem::take(&mut state.pending) {
                eff.send(&state.track_peers, TrackPeersMsg::RetryValidation(p)).await;
            }
            let poll = Duration::from_millis(state.poll_interval_ms.max(1));
            eff.schedule_after(Poll, poll).await;
        }
    }
    state
}
