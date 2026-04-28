// Copyright 2026 PRAGMA
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};

use amaru_kernel::Peer;
use amaru_protocols::manager::ManagerMessage;
use pure_stage::{
    DeserializerGuards, Effect, Instant, Name, ScheduleId, ScheduleIds, StageGraph, StageRef,
    simulation::{SimulationBuilder, SimulationRunning},
    trace_buffer::{TraceBuffer, TraceEntry},
};
use tokio::runtime::{Builder, Runtime};
use tracing::Level;
use tracing_subscriber::util::SubscriberInitExt;

use super::*;
use crate::stages::test_utils::{BufferWriter, Logs};

pub const COOLDOWN_SECS: u64 = 1;

pub fn cooldown_duration() -> Duration {
    Duration::from_secs(COOLDOWN_SECS)
}

pub fn cooldown_instant() -> Instant {
    Instant::at_offset(cooldown_duration())
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
    pub manager: StageRef<ManagerMessage>,
}

impl TestPrep {
    pub fn peer(name: &str) -> Peer {
        Peer::new(name)
    }

    pub fn state_with_timers(&self, timers: BTreeMap<Peer, ScheduleId>) -> PeerSelection {
        PeerSelection {
            manager: self.manager.clone(),
            static_peers: self.state.static_peers.clone(),
            peer_removal_cooldown: self.state.peer_removal_cooldown,
            cooldown_timers: timers,
            downstream_connected: self.state.downstream_connected.clone(),
        }
    }

    pub fn state_with_downstream(&self, downstream: BTreeSet<Peer>) -> PeerSelection {
        PeerSelection {
            manager: self.manager.clone(),
            static_peers: self.state.static_peers.clone(),
            peer_removal_cooldown: self.state.peer_removal_cooldown,
            cooldown_timers: self.state.cooldown_timers.clone(),
            downstream_connected: downstream,
        }
    }
}

pub fn test_prep_static(static_names: &[&str]) -> TestPrep {
    let manager = StageRef::named_for_tests("manager");
    let static_peers: BTreeSet<Peer> = static_names.iter().map(|n| Peer::new(n)).collect();
    let state = PeerSelection::new(manager.clone(), static_peers, COOLDOWN_SECS);
    TestPrep { state, rt: Builder::new_current_thread().build().unwrap(), manager }
}

pub fn test_prep_no_static() -> TestPrep {
    test_prep_static(&[])
}

fn register_guards() -> DeserializerGuards {
    vec![
        pure_stage::register_data_deserializer::<PeerSelection>().boxed(),
        pure_stage::register_data_deserializer::<PeerSelectionMsg>().boxed(),
        pure_stage::register_data_deserializer::<ManagerMessage>().boxed(),
        pure_stage::register_data_deserializer::<ScheduleId>().boxed(),
    ]
}

pub fn setup(prep: &TestPrep, msg: PeerSelectionMsg) -> (SimulationRunning, DeserializerGuards, Logs) {
    setup_preload(prep, [msg])
}

pub fn setup_preload(
    prep: &TestPrep,
    messages: impl IntoIterator<Item = PeerSelectionMsg>,
) -> (SimulationRunning, DeserializerGuards, Logs) {
    let writer = BufferWriter::new();
    let mut logs = writer.clone();

    let sub = tracing_subscriber::fmt()
        .with_max_level(Level::DEBUG)
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .set_default();
    logs.set_guard(sub);

    let guards = register_guards();

    let mut network = SimulationBuilder::default().with_trace_buffer(TraceBuffer::new_shared(100, 1000000));
    let ps = network.stage("ps", stage);
    let ps = network.wire_up(ps, prep.state.clone());
    network.preload(&ps, messages).unwrap();

    let mut running = network.run();
    running.run_until_blocked_incl_effects(prep.rt.handle());

    (running, guards, logs.logs())
}

pub fn te_send(from: impl AsRef<str>, to: impl AsRef<str>, msg: impl pure_stage::SendData) -> TraceEntry {
    TraceEntry::suspend(Effect::send(from, to, Box::new(msg)))
}

pub fn te_schedule(at_stage: impl AsRef<str>, msg: impl pure_stage::SendData, schedule_id: ScheduleId) -> TraceEntry {
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

#[track_caller]
pub fn assert_trace(running: &SimulationRunning, expected: &[TraceEntry]) {
    let mut tb = running.trace_buffer().lock();
    let trace = tb
        .iter_entries()
        .filter_map(|(_, e)| (!matches!(e, TraceEntry::Resume { .. })).then_some(e))
        .collect::<Vec<_>>();
    tb.clear();
    pretty_assertions::assert_eq!(trace, expected);
}
