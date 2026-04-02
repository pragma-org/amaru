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

use amaru_kernel::{BlockHeight, Peer};
use amaru_protocols::chainsync::InitiatorMessage;
use pure_stage::{Effects, StageRef};

use super::ledger_applied_block_height;
use crate::effects::{BaseOps, ConsensusEffects};

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum DeferReqNextMsg {
    Register {
        peer: Peer,
        handler: StageRef<InitiatorMessage>,
        min_ledger_height: BlockHeight,
    },
    /// Drop any deferred `RequestNext` for this peer (e.g. after rollback).
    Cancel {
        peer: Peer,
    },
    Poll,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DeferReqNext {
    pub poll_interval_ms: u64,
    pub pending: Vec<(Peer, StageRef<InitiatorMessage>, BlockHeight)>,
}

impl DeferReqNext {
    pub fn new(poll_interval_ms: u64) -> Self {
        Self { poll_interval_ms, pending: Vec::new() }
    }
}

pub async fn stage(mut state: DeferReqNext, msg: DeferReqNextMsg, eff: Effects<DeferReqNextMsg>) -> DeferReqNext {
    use DeferReqNextMsg::*;
    match msg {
        Register { peer, handler, min_ledger_height } => {
            state.pending.push((peer, handler, min_ledger_height));
        }
        Cancel { peer } => {
            state.pending.retain(|(p, _, _)| p != &peer);
        }
        Poll => {
            dispatch_ready(&mut state, &eff).await;
            let poll = Duration::from_millis(state.poll_interval_ms.max(1));
            ConsensusEffects::new(eff.clone()).base().wait(poll).await;
            eff.send(eff.me_ref(), Poll).await;
        }
    }
    state
}

async fn dispatch_ready(state: &mut DeferReqNext, eff: &Effects<DeferReqNextMsg>) {
    let ops = ConsensusEffects::new(eff.clone());
    let ledger_height = ledger_applied_block_height(&ops);
    let mut remaining = Vec::new();
    for (peer, handler, min_h) in std::mem::take(&mut state.pending) {
        if ledger_height >= min_h {
            eff.send(&handler, InitiatorMessage::RequestNext).await;
        } else {
            remaining.push((peer, handler, min_h));
        }
    }
    state.pending = remaining;
}
