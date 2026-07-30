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

use std::{sync::Arc, time::Duration};

use amaru_kernel::{
    BlockHeader, ConsensusParameters, Epoch, EraHistory, HeaderHash, IsHeader, NetworkName, Peer, Point, Tip,
    make_header, num::CheckedSub,
};
use amaru_ouroboros::ConnectionId;
use amaru_ouroboros_traits::{
    BaseReadChainStore, MockBlockValidator, Nonces, PoolSummaries, WriteChainStore, has_stake_pools::MockHasStakePools,
    in_memory_chain_store::InMemoryChainStore,
};
use amaru_protocols::{
    chainsync::{self, InitiatorMessage},
    manager::ManagerMessage,
    store_effects::{
        GetNoncesEffect, HasHeaderEffect, LoadHeaderEffect, LoadTipEffect, ResourceHeaderStore,
        StoreValidatedHeaderEffect,
    },
};
use amaru_pure_stage::{
    DeserializerGuards, Effect, Instant, Name, ScheduleId, ScheduleIds, StageRef,
    simulation::{SimulationRunning, running::OverrideResult},
    trace_buffer::TraceEntry,
};
use tokio::runtime::{Builder, Handle, Runtime};

use super::{NewTip, TrackPeers, TrackPeersMsg, stage};
use crate::{
    effects::{
        ResourceBlockValidation, ResourceConsensusParameters, ResourceEraHistory, ResourceHasStakePools,
        ResourcePoolSummaries, TipEffect, ValidateHeaderEffect, VolatileTipEffect,
    },
    stages::{
        peer_selection::PeerSelectionMsg,
        test_utils::{
            Logs, SimulationRunMode, StartTimes, TraceMatch, run_simulation_with, start_in_era, tm_external_effect,
        },
    },
};

/// Matches `run_simulation` initial clock (+10s) for schedule id expectations.
pub const SIM_INITIAL_CLOCK_SECS: u64 = 10;

/// Height recheck poll interval used by the stage (`HEIGHT_RECHECK_INTERVAL`).
pub const HEIGHT_RECHECK_INTERVAL: Duration = Duration::from_millis(200);

pub fn height_recheck_schedule_id() -> ScheduleId {
    let when = Instant::at_offset(
        Duration::from_secs(SIM_INITIAL_CLOCK_SECS) + HEIGHT_RECHECK_INTERVAL,
        start_in_era().relative_time,
    );
    ScheduleIds::default().next_at(when)
}

pub fn schedule_id_at(delay_from_sim_start: Duration) -> ScheduleId {
    let when = Instant::at_offset(
        Duration::from_secs(SIM_INITIAL_CLOCK_SECS) + delay_from_sim_start,
        start_in_era().relative_time,
    );
    ScheduleIds::default().next_at(when)
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

pub fn te_clock(instant: Instant) -> TraceEntry {
    TraceEntry::Clock(instant)
}

pub fn build_store(headers: &[BlockHeader]) -> Arc<InMemoryChainStore> {
    let store = Arc::new(InMemoryChainStore::new());
    for header in headers {
        store.store_header(header).unwrap();
    }
    store
}

/// Like [`build_store`] but also persists nonces for each header, mimicking a header that has
/// already been fully validated (as opposed to one merely imported during bootstrap).
pub fn build_store_with_nonces(headers: &[BlockHeader]) -> Arc<InMemoryChainStore> {
    let store = build_store(headers);
    for header in headers {
        store.put_nonces(&header.hash(), &Nonces::for_tests()).unwrap();
    }
    store
}

/// Bundles state, runtime, handler, conn_id, and three linked headers for tests.
pub struct TestPrep {
    pub state: TrackPeers,
    pub rt: Runtime,
    pub handler: StageRef<InitiatorMessage>,
    pub conn_id: ConnectionId,
    /// Three linked headers: [h1, h2, h3] with h1 parent None, h2 parent h1, h3 parent h2.
    pub headers: [BlockHeader; 3],
    pub start_times: StartTimes,
}

impl TestPrep {
    pub fn rt_handle(&self) -> Handle {
        self.rt.handle().clone()
    }
}

/// Creates basic state, runtime, handler, conn_id, and three properly linked headers for tests.
pub fn test_prep() -> TestPrep {
    test_prep_with_max_peer_lead(10_000_000)
}

/// Creates a `TestPrep` with a configurable peer lead limit (for testing defer logic).
pub fn test_prep_with_max_peer_lead(max_peer_lead: u64) -> TestPrep {
    let start_times = start_in_era();
    // EraHistory::default() matches synthetic headers at slots 1,2,3 used throughout these tests.
    // Clock-skew tests map simulation time through this same history (see tests).
    let state = TrackPeers::new(
        EraHistory::default(),
        StageRef::named_for_tests("peer_selection"),
        StageRef::named_for_tests("downstream"),
        max_peer_lead,
        start_times.epoch.checked_sub(Epoch::TWO).unwrap(),
    );
    let rt = Builder::new_current_thread().build().unwrap();
    let handler = StageRef::<InitiatorMessage>::named_for_tests("handler");
    let conn_id = ConnectionId::initial();
    let h1 = make_block_header(1, 1, None);
    let h2 = make_block_header(2, 2, Some(h1.hash()));
    let h3 = make_block_header(3, 3, Some(h2.hash()));

    TestPrep { state, rt, handler, conn_id, headers: [h1, h2, h3], start_times }
}

pub fn make_block_header(block_number: u64, slot: u64, parent: Option<HeaderHash>) -> BlockHeader {
    BlockHeader::from(make_header(block_number, slot, parent))
}

pub fn te_validate_header(at_stage: &str, header: BlockHeader) -> TraceEntry {
    TraceEntry::suspend(Effect::external(at_stage, Box::new(ValidateHeaderEffect::new(&header))))
}

pub fn te_load_tip(at_stage: &str, hash: HeaderHash) -> TraceEntry {
    TraceEntry::suspend(Effect::external(at_stage, Box::new(LoadTipEffect::new(hash))))
}

pub fn te_get_nonces(at_stage: &str, hash: HeaderHash) -> TraceEntry {
    TraceEntry::suspend(Effect::external(at_stage, Box::new(GetNoncesEffect::new(hash))))
}

pub fn te_store_validated_header(at_stage: &str, header: BlockHeader) -> TraceEntry {
    TraceEntry::suspend(Effect::external(
        at_stage,
        Box::new(StoreValidatedHeaderEffect::new(header, Nonces::for_tests())),
    ))
}

pub fn te_record_header_announcement(
    at_stage: &str,
    peer: Peer,
    header: Tip,
    parent: Option<HeaderHash>,
    at: Instant,
) -> TraceEntry {
    TraceEntry::suspend(Effect::external(
        at_stage,
        Box::new(crate::performance::Performance::record_header_announcement(peer, header, parent, at)),
    ))
}

pub fn te_clock_suspend(at_stage: &str) -> TraceEntry {
    TraceEntry::suspend(Effect::Clock { at_stage: Name::from(at_stage) })
}

pub fn tm_volatile_tip(at_stage: &str) -> TraceMatch<'static> {
    tm_external_effect::<VolatileTipEffect>(at_stage)
}

pub fn new_tip(tip: Tip, parent: Point) -> NewTip {
    NewTip { tip, parent, trace_context: Default::default() }
}

fn register_guards() -> DeserializerGuards {
    vec![
        amaru_pure_stage::register_data_deserializer::<TrackPeers>().boxed(),
        amaru_pure_stage::register_data_deserializer::<TrackPeersMsg>().boxed(),
        amaru_pure_stage::register_data_deserializer::<InitiatorMessage>().boxed(),
        amaru_pure_stage::register_data_deserializer::<ManagerMessage>().boxed(),
        amaru_pure_stage::register_data_deserializer::<chainsync::InitiatorResult>().boxed(),
        amaru_pure_stage::register_data_deserializer::<chainsync::InitiatorMessage>().boxed(),
        amaru_pure_stage::register_data_deserializer::<chainsync::HeaderContent>().boxed(),
        amaru_pure_stage::register_data_deserializer::<PeerSelectionMsg>().boxed(),
        amaru_pure_stage::register_data_deserializer::<NewTip>().boxed(),
        amaru_pure_stage::register_data_deserializer::<Tip>().boxed(),
        amaru_pure_stage::register_data_deserializer::<(Tip, Point)>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<LoadHeaderEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<LoadTipEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<HasHeaderEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<GetNoncesEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<StoreValidatedHeaderEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<ValidateHeaderEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<TipEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<VolatileTipEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<crate::performance::RecordHeaderAnnouncementEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<crate::performance::RecordHeaderRejectedEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<crate::performance::RecordIntersectionEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<amaru_protocols::metrics_effects::RecordMetricsEffect>()
            .boxed(),
    ]
}

pub fn setup(
    rt: &Handle,
    state: TrackPeers,
    msg: TrackPeersMsg,
    store: Arc<InMemoryChainStore>,
) -> (SimulationRunning, DeserializerGuards, Logs) {
    setup_base(rt, state, [msg], store, |running| {
        running.override_external_effect::<ValidateHeaderEffect>(usize::MAX, |_| {
            OverrideResult::handled(Ok(Nonces::for_tests()))
        });
    })
}

/// Forces a specific ledger-applied tip and stops when the sim first sleeps on a scheduled
/// wakeup (does not auto-advance time). Safe for frozen-height defer tests that arm a recheck
/// timer without running an infinite height-poll loop.
pub fn setup_with_ledger_tip_until_sleeping(
    rt: &Handle,
    state: TrackPeers,
    msg: impl IntoIterator<Item = TrackPeersMsg>,
    store: Arc<InMemoryChainStore>,
    ledger_tip: Tip,
) -> (SimulationRunning, DeserializerGuards, Logs) {
    setup_base_until_sleeping(rt, state, msg, store, |running| {
        running.override_external_effect::<ValidateHeaderEffect>(usize::MAX, |_| {
            OverrideResult::handled(Ok(Nonces::for_tests()))
        });
        running.override_external_effect::<VolatileTipEffect>(usize::MAX, {
            move |_| OverrideResult::handled(ledger_tip)
        });
        running.override_external_effect::<TipEffect>(usize::MAX, move |_| OverrideResult::handled(ledger_tip));
    })
}

pub fn setup_base(
    rt: &Handle,
    state: TrackPeers,
    msg: impl IntoIterator<Item = TrackPeersMsg>,
    store: Arc<InMemoryChainStore>,
    overrides: impl FnOnce(&mut SimulationRunning),
) -> (SimulationRunning, DeserializerGuards, Logs) {
    setup_inner(rt, state, msg, store, overrides, true)
}

fn setup_base_until_sleeping(
    rt: &Handle,
    state: TrackPeers,
    msg: impl IntoIterator<Item = TrackPeersMsg>,
    store: Arc<InMemoryChainStore>,
    overrides: impl FnOnce(&mut SimulationRunning),
) -> (SimulationRunning, DeserializerGuards, Logs) {
    setup_inner(rt, state, msg, store, overrides, false)
}

fn setup_inner(
    rt: &Handle,
    state: TrackPeers,
    msg: impl IntoIterator<Item = TrackPeersMsg>,
    store: Arc<InMemoryChainStore>,
    overrides: impl FnOnce(&mut SimulationRunning),
    advance_wakeups: bool,
) -> (SimulationRunning, DeserializerGuards, Logs) {
    use amaru_pure_stage::StageGraph;

    let mode = if advance_wakeups { SimulationRunMode::UntilBlocked } else { SimulationRunMode::UntilSleeping };
    run_simulation_with(
        rt,
        register_guards(),
        |network| {
            let tp = network.stage("tp", stage);
            let tp = network.wire_up(tp, state);
            network.preload(&tp, msg).unwrap();
        },
        |resources| {
            resources.put::<crate::performance::ResourcePerformance>(std::sync::Arc::new(
                crate::performance::Performance::new(),
            ));
            resources.put::<ResourceHeaderStore>(store.clone());
            let block_validation = Arc::new(MockBlockValidator::new(store.get_best_chain_tip().point()));
            resources.put::<ResourceBlockValidation>(block_validation.clone());
            resources.put::<ResourceHasStakePools>(Arc::new(MockHasStakePools));
            let era_history = NetworkName::Preprod.as_era_history().expect("preprod era for tests").clone();
            let global = NetworkName::Preprod.as_global_parameters().expect("preprod global for tests").clone();
            let cp = Arc::new(ConsensusParameters::new(global, &era_history));
            resources.put::<ResourceConsensusParameters>(cp);
            resources.put::<ResourceEraHistory>(era_history);
            resources.put::<ResourcePoolSummaries>(Arc::new(PoolSummaries::default()));
        },
        overrides,
        mode,
    )
}
