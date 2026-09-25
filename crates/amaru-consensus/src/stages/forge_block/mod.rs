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

//! Block production stage.
//!
//! Wired from `build_node` when forging credentials are installed on the node.
//! The product binary leaves that resource empty, so the stage is not in the graph.
//!
//! The typestate remainder in `protocol` is the audit surface for messages,
//! timers, and forging effects. Internal decisions live in `calc`.

mod calc;
#[cfg(any(test, feature = "test-utils"))]
mod credentials;
mod effects;
mod protocol;
mod schedule;

use amaru_kernel::{ConsensusParameters, KesPeriod, Point, PoolId, ProtocolVersion};
pub use amaru_ouroboros_traits::ForgingCredentials;
use amaru_pure_stage::{ScheduleId, StageRef, typestate::prelude::*};
pub use calc::FreezeWatch;
#[cfg(any(test, feature = "test-utils"))]
pub use credentials::{TEST_COLD_KEY, TEST_VRF_SEED, TestCredentials, test_vrf_key};
pub use effects::{
    ForgeEffectError, ForgeHeaderEffect, ForgedBody, LeaderScheduleEffect, ResourceForgingCredentials,
    TakeForForgeEffect,
};
pub use protocol::{AdoptedTip, DueLead, ForgeBlockMsg, LeaderSchedule, Live, SelectChainOut, stage};
use schedule::Schedule;

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
    pub ocert_start_period: KesPeriod,
    /// Ouroboros system start, as Unix time in milliseconds.
    pub system_start_unix_ms: u64,
    pub protocol_version: ProtocolVersion,
    pub adopted_tip: Point,
    pub adopted_parent: Point,
    pub schedule: Schedule,
    pub next_lead: Option<ScheduleId>,
    /// Identifies the latest scheduling decision. A `DueLead` queued under an older
    /// value is ignored, including one whose timer `cancel_schedule` could not stop.
    pub schedule_generation: u64,
    pub freeze: Option<FreezeWatch>,
}

impl ForgeBlock {
    pub fn new(
        select_chain: StageRef<SelectChainMsg>,
        consensus_parameters: ConsensusParameters,
        system_start_unix_ms: u64,
        k: u64,
        pool: PoolId,
        ocert_start_period: KesPeriod,
        protocol_version: ProtocolVersion,
    ) -> Self {
        Self {
            live: initial_state::<protocol::Idle>().into(),
            data: ForgeData {
                select_chain: SelectChainOut::new(select_chain),
                consensus_parameters,
                k,
                pool,
                ocert_start_period,
                system_start_unix_ms,
                protocol_version,
                adopted_tip: Point::Origin,
                adopted_parent: Point::Origin,
                schedule: Default::default(),
                next_lead: None,
                schedule_generation: 0,
                freeze: None,
            },
        }
    }
}

#[cfg(test)]
mod test_setup;
#[cfg(test)]
mod tests;
