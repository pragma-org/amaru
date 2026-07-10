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

//! Sub-stage used by [`super::stage`]: delays [`InitiatorMessage::RequestNext`](amaru_protocols::chainsync::InitiatorMessage::RequestNext)
//! until the ledger applied tip has advanced far enough.

use std::time::Duration;

use amaru_kernel::Slot;
use amaru_protocols::chainsync::InitiatorMessage;
use amaru_pure_stage::{Effects, StageRef};

use super::ledger_applied_slot;
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum DeferReqNextMsg {
    Register { handler: StageRef<InitiatorMessage>, min_ledger_slot: Slot },
    Poll,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DeferReqNext {
    pub poll_interval_ms: u64,
    pub pending: Vec<(StageRef<InitiatorMessage>, Slot)>,
}

impl DeferReqNext {
    pub fn new(poll_interval_ms: u64) -> Self {
        Self { poll_interval_ms, pending: Vec::new() }
    }
}

pub async fn stage(mut state: DeferReqNext, msg: DeferReqNextMsg, eff: Effects<DeferReqNextMsg>) -> DeferReqNext {
    use DeferReqNextMsg::*;
    match msg {
        Register { handler, min_ledger_slot } => {
            state.pending.push((handler, min_ledger_slot));
        }
        Poll => {
            dispatch_ready(&mut state, &eff).await;
            let poll = Duration::from_millis(state.poll_interval_ms.max(1));
            eff.schedule_after(Poll, poll).await;
        }
    }
    state
}

async fn dispatch_ready(state: &mut DeferReqNext, eff: &Effects<DeferReqNextMsg>) {
    let ledger_slot = ledger_applied_slot(eff).await;
    let mut remaining = Vec::new();
    for (handler, min_slot) in std::mem::take(&mut state.pending) {
        if ledger_slot >= min_slot {
            eff.send(&handler, InitiatorMessage::RequestNext).await;
        } else {
            remaining.push((handler, min_slot));
        }
    }
    state.pending = remaining;
}
