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

use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};

use amaru_kernel::{Peer, PeerCandidate, Point};
use amaru_protocols::manager::ManagerMessage;
use amaru_pure_stage::{
    DeserializerGuards, Effect, Instant, Name, ScheduleId, ScheduleIds, StageGraph, StageRef,
    simulation::{SimulationRunning, running::OverrideResult},
    trace_buffer::TraceEntry,
};
use tokio::runtime::Runtime;

use super::*;
pub use crate::stages::test_utils::TraceMatch;
use crate::{
    effects::{GenerateRandomSeed, RegisteredRelaySocketAddrsEffect, TipEffect, VolatileTipEffect},
    stages::test_utils::{Logs, SimulationRunMode, run_simulation_with, start_in_era},
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
    Instant::at_offset(Duration::from_secs(SIM_INITIAL_CLOCK_SECS) + cooldown_duration(), start_in_era().relative_time)
}

/// Absolute time `delay` after simulation t0.
pub fn sim_at(delay: Duration) -> Instant {
    Instant::at_offset(Duration::from_secs(SIM_INITIAL_CLOCK_SECS) + delay, start_in_era().relative_time)
}

pub fn first_schedule_id() -> ScheduleId {
    ScheduleIds::default().next_at(cooldown_instant())
}

/// End time for a static-peer ban started at simulation t0 (`STATIC_PEER_BAN_PERIOD` = 10s).
pub fn static_cooldown_instant() -> Instant {
    Instant::at_offset(Duration::from_secs(SIM_INITIAL_CLOCK_SECS + 10), start_in_era().relative_time)
}

pub fn first_static_schedule_id() -> ScheduleId {
    ScheduleIds::default().next_at(static_cooldown_instant())
}

/// Second schedule id after a prior schedule at `prior_when` (counter advances once).
pub fn second_schedule_id_at(when: Instant) -> ScheduleId {
    let ids = ScheduleIds::default();
    let _ = ids.next_at(when);
    ids.next_at(when)
}

/// Build the cool-down fields after a single non-static ban armed at `cooldown_instant()`.
pub fn with_single_cooldown(state: &mut PeerSelection, peer: Peer, schedule_id: ScheduleId) {
    state.cooldowns.add_and_is_first(peer, cooldown_instant());
    state.cooldown_timer = Some(schedule_id);
}

pub fn te_clock_suspend(at_stage: impl AsRef<str>) -> TraceEntry {
    TraceEntry::suspend(Effect::Clock { at_stage: Name::from(at_stage.as_ref()) })
}

pub struct TestPrep {
    pub state: PeerSelection,
    pub rt: Runtime,
    /// Seeded into the Performance resource at simulation start (not stage state).
    pub static_peers: BTreeSet<Peer>,
    pub extra_static: BTreeSet<PeerCandidate>,
    pub snapshot_candidates: BTreeSet<Peer>,
    pub ledger_candidates: BTreeSet<Peer>,
    pub peer_mix: crate::performance::PeerMix,
    /// Mock DNS: `ResolvePeerCandidate` returns these peers (or empty if absent).
    pub resolve: BTreeMap<PeerCandidate, BTreeSet<Peer>>,
}

impl TestPrep {
    pub fn peer(name: &str) -> Peer {
        name.parse().unwrap_or_else(|e| panic!("test peer {name:?} must be a literal IP:port: {e}"))
    }

    /// Seed ledger candidates for the Performance resource installed in [`setup_preload`].
    pub fn with_ledger(mut self, names: &[&str]) -> Self {
        self.ledger_candidates = names.iter().map(|n| Self::peer(n)).collect();
        self
    }
}

pub fn test_prep(static_names: &[&str]) -> TestPrep {
    test_prep_with_snapshot(static_names, &[])
}

pub fn test_prep_with_snapshot(static_names: &[&str], snapshot_names: &[&str]) -> TestPrep {
    let manager = StageRef::named_for_tests("manager");
    let static_peers: BTreeSet<Peer> = static_names.iter().map(|n| TestPrep::peer(n)).collect();
    let snapshot_candidates: BTreeSet<Peer> = snapshot_names.iter().map(|n| TestPrep::peer(n)).collect();
    let peer_mix = crate::performance::PeerMix::default();
    let state = PeerSelection::new(manager, 3, 10, COOLDOWN_SECS);
    TestPrep {
        state,
        rt: crate::stages::test_utils::test_runtime(),
        static_peers,
        extra_static: BTreeSet::new(),
        snapshot_candidates,
        ledger_candidates: BTreeSet::new(),
        peer_mix,
        resolve: BTreeMap::new(),
    }
}

pub fn register_guards() -> DeserializerGuards {
    vec![
        amaru_pure_stage::register_data_deserializer::<PeerSelection>().boxed(),
        amaru_pure_stage::register_data_deserializer::<PeerSelectionMsg>().boxed(),
        amaru_pure_stage::register_data_deserializer::<ManagerMessage>().boxed(),
        amaru_pure_stage::register_data_deserializer::<ScheduleId>().boxed(),
        amaru_pure_stage::register_data_deserializer::<amaru_protocols::peer_sharing::ShareResult>().boxed(),
        amaru_pure_stage::register_data_deserializer::<amaru_protocols::peer_sharing::SharePeersReply>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<GenerateRandomSeed>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<crate::performance::ClearPeerAvailabilityEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<crate::performance::PeerAdversarialEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<crate::performance::RecordAdvertisabilityEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<crate::performance::RecordConnectionFailureEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<crate::performance::OkForSharingEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<crate::performance::SelectOutboundEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<crate::performance::SelectSharePeersEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<crate::performance::IsStaticPeerEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<crate::performance::StaticPeersEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<crate::performance::UnresolvedStaticEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<crate::performance::UnresolvedSnapshotEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<crate::performance::IngestResolvedEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<crate::effects::ResolvePeerCandidate>().boxed(),
        amaru_pure_stage::register_data_deserializer::<crate::effects::ResolvePeerCandidateResult>().boxed(),
        amaru_pure_stage::register_data_deserializer::<PeerCandidate>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<crate::performance::SourceCountsEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<crate::performance::IngestSharedPeersEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<crate::performance::SetLedgerCandidatesEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<crate::performance::SharedContainsEffect>().boxed(),
    ]
}

/// Simulation clock at t0 (matches `run_simulation` initial clock of +10s).
pub fn sim_t0() -> Instant {
    Instant::at_offset(Duration::from_secs(SIM_INITIAL_CLOCK_SECS), start_in_era().relative_time)
}

pub fn te_peer_adversarial(at_stage: &str, peer: Peer) -> TraceEntry {
    TraceEntry::suspend(Effect::external(
        at_stage,
        Box::new(crate::performance::Performance::peer_adversarial(peer, sim_t0())),
    ))
}

pub fn te_is_static_peer(at_stage: &str, peer: Peer) -> TraceEntry {
    TraceEntry::suspend(Effect::external(at_stage, Box::new(crate::performance::Performance::is_static_peer(peer))))
}

pub fn te_clear_peer_availability(at_stage: &str, peer: Peer) -> TraceEntry {
    TraceEntry::suspend(Effect::external(
        at_stage,
        Box::new(crate::performance::Performance::clear_peer_availability(peer)),
    ))
}

pub fn te_record_advertisability(at_stage: &str, peer: Peer, advertisable: bool, at: Instant) -> TraceEntry {
    TraceEntry::suspend(Effect::external(
        at_stage,
        Box::new(crate::performance::Performance::record_advertisability(peer, advertisable, at)),
    ))
}

pub fn te_record_connection_failure(at_stage: &str, peer: Peer, at: Instant) -> TraceEntry {
    TraceEntry::suspend(Effect::external(
        at_stage,
        Box::new(crate::performance::Performance::record_connection_failure(peer, at)),
    ))
}

pub fn setup(prep: &TestPrep, msg: PeerSelectionMsg) -> (SimulationRunning, DeserializerGuards, Logs) {
    setup_preload(prep, [msg])
}

pub fn setup_preload(
    prep: &TestPrep,
    messages: impl IntoIterator<Item = PeerSelectionMsg>,
) -> (SimulationRunning, DeserializerGuards, Logs) {
    setup_preload_with_mode(prep, messages, SimulationRunMode::UntilBlocked)
}

/// Like [`setup_preload`], but stops at the first scheduled wakeup without advancing time.
/// Used by tests that need to inject messages while a cool-down timer is still pending.
pub fn setup_preload_until_sleeping(
    prep: &TestPrep,
    messages: impl IntoIterator<Item = PeerSelectionMsg>,
) -> (SimulationRunning, DeserializerGuards, Logs) {
    setup_preload_with_mode(prep, messages, SimulationRunMode::UntilSleeping)
}

fn setup_preload_with_mode(
    prep: &TestPrep,
    messages: impl IntoIterator<Item = PeerSelectionMsg>,
    mode: SimulationRunMode,
) -> (SimulationRunning, DeserializerGuards, Logs) {
    let guards = register_guards();

    run_simulation_with(
        prep.rt.handle(),
        guards,
        |network| {
            // Larger bulk mailbox so tests can preload many adversarial messages without
            // hitting the default size of 10 (the cool-down fix is about the *priority*
            // mailbox, not bulk preload).
            let mut network = network.with_mailbox_size(64);
            let ps = network.stage("ps", stage);
            let ps = network.wire_up(ps, prep.state.clone());
            network.preload(&ps, messages).unwrap();
            network
        },
        |resources| {
            resources.put::<crate::performance::ResourcePerformance>(std::sync::Arc::new(
                crate::performance::Performance::with_peer_sources(
                    prep.static_peers
                        .iter()
                        .copied()
                        .map(PeerCandidate::from)
                        .chain(prep.extra_static.iter().cloned())
                        .collect(),
                    prep.snapshot_candidates.iter().copied().map(PeerCandidate::from).collect(),
                    prep.ledger_candidates.clone(),
                    prep.peer_mix.clone(),
                ),
            ));
        },
        |running| {
            running.use_virtual_child_stages(true);

            running
                .override_external_effect::<VolatileTipEffect>(usize::MAX, |_| OverrideResult::handled(Point::Origin));
            running.override_external_effect::<TipEffect>(usize::MAX, |_| OverrideResult::handled(Point::Origin));
            running.override_external_effect::<RegisteredRelaySocketAddrsEffect>(usize::MAX, |_| {
                OverrideResult::handled(Ok(BTreeSet::new()))
            });

            // NOTE: This makes peer selection's random choices fully deterministic in tests.
            running
                .override_external_effect::<GenerateRandomSeed>(usize::MAX, |_| OverrideResult::handled([0x42u8; 32]));
            let resolve = prep.resolve.clone();
            running.override_external_effect::<crate::effects::ResolvePeerCandidate>(usize::MAX, move |eff| {
                let peers = resolve.get(&eff.candidate).cloned().unwrap_or_default();
                OverrideResult::handled(crate::effects::ResolvePeerCandidateResult {
                    candidate: eff.candidate.clone(),
                    origin: eff.origin,
                    peers,
                })
            });
            // Peer-sharing filters: treat all candidates as shareable unless a test overrides.
            running.override_external_effect::<crate::performance::OkForSharingEffect>(usize::MAX, |_| {
                OverrideResult::handled(true)
            });
        },
        mode,
    )
}

/// Stage ref matching the wired `ps` stage in [`setup_preload`].
pub fn peer_selection_stage() -> StageRef<PeerSelectionMsg> {
    StageRef::named_for_tests("ps-1")
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
