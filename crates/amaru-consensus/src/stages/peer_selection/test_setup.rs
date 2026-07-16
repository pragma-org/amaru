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

use std::{collections::BTreeSet, time::Duration};

use amaru_kernel::{Peer, Tip};
use amaru_protocols::manager::ManagerMessage;
use amaru_pure_stage::{
    DeserializerGuards, Effect, Instant, Name, ScheduleId, ScheduleIds, StageGraph, StageRef,
    simulation::{SimulationRunning, running::OverrideResult},
    trace_buffer::TraceEntry,
};
use tokio::runtime::{Builder, Runtime};

use super::*;
pub use crate::stages::test_utils::TraceMatch;
use crate::{
    effects::{GenerateRandomSeed, RegisteredRelaySocketAddrsEffect, TipEffect, VolatileTipEffect},
    stages::test_utils::{Logs, run_simulation},
};

/// Matches an `AddStage` effect whose generated name starts with the given prefix.
/// Useful for testing first-message child wiring when the simulation appends a suffix
/// (e.g. "peer-selection/ledger-check-2").
pub fn tm_add_stage_starts_with(prefix: &str) -> TraceMatch<'static> {
    let prefix = prefix.to_string();
    let description = format!("AddStage name starts with {}", prefix);
    TraceMatch::Property(
        Box::new(move |e| {
            let TraceEntry::Suspend(Effect::AddStage { name, .. }) = e else {
                return false;
            };
            name.as_str().starts_with(&prefix)
        }),
        description,
    )
}

pub const COOLDOWN_SECS: u64 = 1;

/// Matches `run_simulation`'s `with_initial_clock(Instant::at_offset(10s))`.
pub const SIM_INITIAL_CLOCK_SECS: u64 = 10;

pub fn cooldown_duration() -> Duration {
    Duration::from_secs(COOLDOWN_SECS)
}

/// Absolute schedule time when a cooldown started at simulation t0 ends.
///
/// Matches `run_simulation`: initial clock at +10s and `global_epoch_offset` from `start_in_era()`.
pub fn cooldown_instant() -> Instant {
    use crate::stages::test_utils::start_in_era;
    Instant::at_offset(Duration::from_secs(SIM_INITIAL_CLOCK_SECS) + cooldown_duration(), start_in_era().relative_time)
}

pub fn first_schedule_id() -> ScheduleId {
    ScheduleIds::default().next_at(cooldown_instant())
}

pub fn second_schedule_id() -> ScheduleId {
    let ids = ScheduleIds::default();
    let _ = ids.next_at(cooldown_instant());
    ids.next_at(cooldown_instant())
}

pub struct TestPrep {
    pub state: PeerSelection,
    pub rt: Runtime,
}

impl TestPrep {
    pub fn peer(name: &str) -> Peer {
        Peer::new(name)
    }
}

pub fn test_prep(static_names: &[&str]) -> TestPrep {
    let manager = StageRef::named_for_tests("manager");
    let static_peers: BTreeSet<Peer> = static_names.iter().map(|n| Peer::new(n)).collect();
    let state = PeerSelection::new(manager, static_peers, 3, 10, COOLDOWN_SECS);
    TestPrep { state, rt: Builder::new_current_thread().build().unwrap() }
}

pub fn register_guards() -> DeserializerGuards {
    vec![
        amaru_pure_stage::register_data_deserializer::<PeerSelection>().boxed(),
        amaru_pure_stage::register_data_deserializer::<PeerSelectionMsg>().boxed(),
        amaru_pure_stage::register_data_deserializer::<ManagerMessage>().boxed(),
        amaru_pure_stage::register_data_deserializer::<ScheduleId>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<GenerateRandomSeed>().boxed(),
    ]
}

pub fn setup(prep: &TestPrep, msg: PeerSelectionMsg) -> (SimulationRunning, DeserializerGuards, Logs) {
    setup_preload(prep, [msg])
}

pub fn setup_preload(
    prep: &TestPrep,
    messages: impl IntoIterator<Item = PeerSelectionMsg>,
) -> (SimulationRunning, DeserializerGuards, Logs) {
    let guards = register_guards();

    run_simulation(
        prep.rt.handle(),
        guards,
        |network| {
            let ps = network.stage("ps", stage);
            let ps = network.wire_up(ps, prep.state.clone());
            network.preload(&ps, messages).unwrap();
        },
        |_resources| {
            // Resources (if any) would go here. Ledger effects are overridden below.
        },
        |running| {
            running.use_virtual_child_stages(true);

            // Override ledger external effects so the internal "peer-selection/ledger-check"
            // child created on Initialize does not require a real Ledger resource.
            running
                .override_external_effect::<VolatileTipEffect>(usize::MAX, |_| OverrideResult::handled(Tip::origin()));
            running.override_external_effect::<TipEffect>(usize::MAX, |_| OverrideResult::handled(Tip::origin()));
            running.override_external_effect::<RegisteredRelaySocketAddrsEffect>(usize::MAX, |_| {
                OverrideResult::handled(Ok(BTreeSet::new()))
            });

            // Make peer selection's random choices fully deterministic in tests.
            running
                .override_external_effect::<GenerateRandomSeed>(usize::MAX, |_| OverrideResult::handled([0x42u8; 32]));
        },
    )
}

pub fn te_send(from: impl AsRef<str>, to: impl AsRef<str>, msg: impl amaru_pure_stage::SendData) -> TraceEntry {
    TraceEntry::suspend(Effect::send(from, to, Box::new(msg)))
}

pub fn te_schedule(
    at_stage: impl AsRef<str>,
    msg: impl amaru_pure_stage::SendData,
    schedule_id: ScheduleId,
) -> TraceEntry {
    TraceEntry::suspend(Effect::Schedule {
        at_stage: Name::from(at_stage.as_ref()),
        msg: Box::new(msg),
        id: schedule_id,
    })
}

pub fn te_cancel_schedule(at_stage: impl AsRef<str>, schedule_id: ScheduleId) -> TraceEntry {
    TraceEntry::suspend(Effect::CancelSchedule { at_stage: Name::from(at_stage.as_ref()), id: schedule_id })
}

pub fn te_clock(instant: Instant) -> TraceEntry {
    TraceEntry::Clock(instant)
}

pub fn te_random_seed(at_stage: impl AsRef<str>) -> TraceEntry {
    TraceEntry::Suspend(Effect::External {
        at_stage: Name::from(at_stage.as_ref()),
        effect: Box::new(GenerateRandomSeed),
    })
}
