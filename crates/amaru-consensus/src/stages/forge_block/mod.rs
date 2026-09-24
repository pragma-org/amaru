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

//! Block production stage. Not wired into the consensus graph.
//!
//! The typestate remainder in [`protocol`] is the audit surface for messages,
//! timers, and forging effects. Internal decisions live in [`calc`].

mod calc;
mod effects;
mod protocol;

use std::collections::BTreeSet;

use amaru_kernel::{ConsensusParameters, Epoch, Point, PoolId, Slot};
use amaru_pure_stage::{ScheduleId, StageRef, typestate::prelude::*};
pub use calc::FreezeWatch;
pub use effects::{ForgeEffectError, ForgeHeaderEffect, ForgedBody, LeaderScheduleEffect, TakeForForgeEffect};
pub use protocol::{AdoptedTip, ForgeBlockMsg, LeadSlot, LeaderSchedule, Live, SelectChainOut, stage};

use crate::stages::select_chain::SelectChainMsg;

/// Block forging stage state.
///
/// [`Live`] sits beside [`ForgeData`] so a handler can take the protocol token
/// by value and still mutably borrow the rest. The token put back is the one
/// `finish` returns.
///
/// See EDR035 for more details on wiring and internal function.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ForgeBlock {
    pub live: Live,
    pub data: ForgeData,
}

/// Forging context that persists across mailbox messages.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ForgeData {
    pub select_chain: SelectChainOut,
    pub consensus_parameters: ConsensusParameters,
    pub k: u64,
    pub pool: PoolId,
    pub ocert_start_period: u64,
    pub adopted_tip: Point,
    pub adopted_parent: Point,
    pub led_slots: Vec<Slot>,
    pub next_lead: Option<ScheduleId>,
    /// Identifies the latest scheduling decision. A `LeadSlot` queued under an older
    /// value is ignored, including one whose timer `cancel_schedule` could not stop.
    pub schedule_generation: u64,
    pub freeze: Option<FreezeWatch>,
    pub pending_epochs: BTreeSet<Epoch>,
    /// Next epoch whose schedule we computed from a freeze, if any.
    pub predicted_epoch: Option<Epoch>,
}

impl ForgeBlock {
    pub fn new(
        select_chain: StageRef<SelectChainMsg>,
        consensus_parameters: ConsensusParameters,
        k: u64,
        pool: PoolId,
        ocert_start_period: u64,
    ) -> Self {
        Self {
            live: initial_state::<protocol::Idle>().into(),
            data: ForgeData {
                select_chain: SelectChainOut::new(select_chain),
                consensus_parameters,
                k,
                pool,
                ocert_start_period,
                adopted_tip: Point::Origin,
                adopted_parent: Point::Origin,
                led_slots: Vec::new(),
                next_lead: None,
                schedule_generation: 0,
                freeze: None,
                pending_epochs: BTreeSet::new(),
                predicted_epoch: None,
            },
        }
    }
}

#[cfg(test)]
mod test_setup;
#[cfg(test)]
mod tests;
