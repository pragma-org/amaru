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
use amaru_ouroboros::{
    ConnectionDirection, ConnectionId, ConnectionRecord, ObservedAt, PeerTracking, RemoteInitiators,
};
use amaru_protocols::{connection::MiniProtocolUse, manager::ManagerMessage};
use amaru_pure_stage::{
    DeserializerGuards, Instant, ScheduleId, StageGraph, StageRef,
    simulation::{SimulationRunning, running::OverrideResult},
};
use tokio::runtime::Runtime;

use super::*;
use crate::{
    effects::{GenerateRandomSeed, RegisteredRelayCandidatesEffect, TipEffect, VolatileTipEffect},
    stages::test_utils::{Logs, SimulationRunMode, run_simulation_with, start_in_era},
};

pub const COOLDOWN_SECS: u64 = 1;

/// Ban duration passed to [`PeerSelection::new`] in these tests.
pub fn cooldown_duration() -> Duration {
    Duration::from_secs(COOLDOWN_SECS)
}

/// Matches `run_simulation`'s `with_initial_clock(Instant::at_offset(10s))`.
pub const SIM_INITIAL_CLOCK_SECS: u64 = 10;

pub struct TestPrep {
    pub state: PeerSelection,
    pub rt: Runtime,
    /// Seeded into the Performance resource at simulation start (not stage state).
    pub static_peers: BTreeSet<Peer>,
    pub extra_static: BTreeSet<PeerCandidate>,
    pub snapshot_candidates: BTreeSet<Peer>,
    pub ledger_candidates: BTreeSet<PeerCandidate>,
    pub peer_mix: crate::performance::PeerMix,
    /// Mock DNS: `ResolvePeerCandidate` returns the first peer in the set (or `None` if absent/empty).
    pub resolve: BTreeMap<PeerCandidate, BTreeSet<Peer>>,
    /// When set, `QueryPeerView` returns this view instead of reading the resource.
    pub scripted_view: Option<crate::performance::PeerView>,
    /// Recorded on the resource before the stage runs. A new candidate bumps the generation.
    pub learned_share: Option<(Peer, std::net::SocketAddr)>,
    /// Established outbound bearer recorded before the stage runs.
    pub established: Option<(Peer, ConnectionId)>,
    /// Extra established bearers recorded before the stage runs.
    pub established_connections: Vec<ConnectionRecord>,
    /// Intersection-not-found mark recorded before the stage runs.
    pub uninteresting: Option<(Peer, ConnectionId, bool)>,
    /// Replaces ledger candidates before the stage runs, bumping the generation.
    pub ledger_write: Option<BTreeSet<PeerCandidate>>,
}

impl TestPrep {
    pub fn peer(name: &str) -> Peer {
        name.parse().unwrap_or_else(|e| panic!("test peer {name:?} must be a literal IP:port: {e}"))
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
    let state = PeerSelection::new(manager, 3, 10, cooldown_duration());
    TestPrep {
        state,
        rt: crate::stages::test_utils::test_runtime(),
        static_peers,
        extra_static: BTreeSet::new(),
        snapshot_candidates,
        ledger_candidates: BTreeSet::new(),
        peer_mix,
        resolve: BTreeMap::new(),
        scripted_view: None,
        learned_share: None,
        established: None,
        established_connections: Vec::new(),
        uninteresting: None,
        ledger_write: None,
    }
}

pub fn register_guards() -> DeserializerGuards {
    vec![
        amaru_pure_stage::register_data_deserializer::<PeerSelection>().boxed(),
        amaru_pure_stage::register_data_deserializer::<PeerSelectionMsg>().boxed(),
        amaru_pure_stage::register_data_deserializer::<ManagerMessage>().boxed(),
        amaru_pure_stage::register_data_deserializer::<ScheduleId>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<GenerateRandomSeed>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<crate::performance::ClearPeerAvailabilityEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<crate::performance::PeerAdversarialEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<crate::performance::RecordAdvertisabilityEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<crate::performance::RecordConnectionFailureEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<crate::performance::RankPeersForChurnEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<crate::performance::OkForSharingEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<crate::performance::SelectOutboundEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<crate::performance::QueryPeerViewEffect>().boxed(),
        amaru_pure_stage::register_data_deserializer::<crate::performance::PeerView>().boxed(),
        amaru_pure_stage::register_data_deserializer::<Option<crate::performance::PeerView>>().boxed(),
        amaru_pure_stage::register_data_deserializer::<crate::performance::SelectUsing>().boxed(),
        amaru_pure_stage::register_data_deserializer::<crate::performance::OutboundPick>().boxed(),
        amaru_pure_stage::register_data_deserializer::<Vec<std::net::SocketAddr>>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<crate::performance::IsStaticPeerEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<crate::performance::NoteDialEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<crate::effects::ResolvePeerCandidate>().boxed(),
        amaru_pure_stage::register_data_deserializer::<crate::effects::ResolvePeerCandidateResult>().boxed(),
        amaru_pure_stage::register_data_deserializer::<PeerCandidate>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<crate::performance::SourceCountsEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<crate::performance::EvictRecordsEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<crate::performance::SetLedgerCandidatesEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<crate::performance::SharedContainsEffect>().boxed(),
    ]
}

/// Simulation clock at t0 (matches `run_simulation` initial clock of +10s).
pub fn sim_t0() -> Instant {
    Instant::at_offset(Duration::from_secs(SIM_INITIAL_CLOCK_SECS), start_in_era().relative_time)
}

pub fn setup(prep: &TestPrep, msg: PeerSelectionMsg) -> (SimulationRunning, DeserializerGuards, Logs) {
    setup_preload(prep, [msg])
}

pub fn setup_preload(
    prep: &TestPrep,
    messages: impl IntoIterator<Item = PeerSelectionMsg>,
) -> (SimulationRunning, DeserializerGuards, Logs) {
    // Elapse sampled effect durations, then stop at the first stage timer (churn) without firing it.
    setup_preload_with_mode(prep, messages, SimulationRunMode::UntilStageTimer)
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
        |mut network| {
            let ps = network.stage("ps", stage);
            let ps = network.wire_up(ps, prep.state.clone());
            network.preload(&ps, messages).unwrap();
            network
        },
        |resources| {
            let performance = crate::performance::Performance::with_peer_sources(
                prep.static_peers
                    .iter()
                    .copied()
                    .map(PeerCandidate::from)
                    .chain(prep.extra_static.iter().cloned())
                    .collect(),
                prep.snapshot_candidates.iter().copied().map(PeerCandidate::from).collect(),
                prep.ledger_candidates.clone(),
                prep.peer_mix.clone(),
            );
            if let Some((donor, addr)) = prep.learned_share {
                prep.rt.block_on(performance.record_shared_peers(
                    donor,
                    vec![addr],
                    ObservedAt::new(Duration::ZERO, Duration::ZERO),
                ));
            }
            let observed = ObservedAt::new(Duration::from_secs(SIM_INITIAL_CLOCK_SECS), start_in_era().relative_time);
            if let Some((peer, conn_id)) = prep.established {
                prep.rt.block_on(performance.record_connection_established(
                    ConnectionRecord {
                        peer,
                        conn_id,
                        direction: ConnectionDirection::Outbound,
                        full_duplex: false,
                        advertisable: false,
                        local_use: MiniProtocolUse::Diffusion,
                        remote_initiators: RemoteInitiators::default(),
                        established_at: observed,
                    },
                    observed,
                ));
            }
            for conn in &prep.established_connections {
                let at = conn.established_at;
                prep.rt.block_on(performance.record_connection_established(conn.clone(), at));
            }
            if let Some((peer, conn_id, after_rollback)) = prep.uninteresting {
                performance.record_uninteresting(peer, conn_id, after_rollback, observed);
            }
            if let Some(candidates) = prep.ledger_write.clone() {
                performance.testing_replace_ledger_candidates(candidates);
            }
            resources.put::<crate::performance::ResourcePerformance>(std::sync::Arc::new(performance));
        },
        |running| {
            running.use_virtual_child_stages(true);

            running
                .override_external_effect::<VolatileTipEffect>(usize::MAX, |_| OverrideResult::handled(Point::Origin));
            running.override_external_effect::<TipEffect>(usize::MAX, |_| OverrideResult::handled(Point::Origin));
            running.override_external_effect::<RegisteredRelayCandidatesEffect>(usize::MAX, |_| {
                OverrideResult::handled(Ok(BTreeSet::new()))
            });

            // NOTE: This makes peer selection's random choices fully deterministic in tests.
            running
                .override_external_effect::<GenerateRandomSeed>(usize::MAX, |_| OverrideResult::handled([0x42u8; 32]));
            if let Some(view) = prep.scripted_view.clone() {
                running.override_external_effect::<crate::performance::QueryPeerViewEffect>(usize::MAX, move |_| {
                    OverrideResult::handled(Some(view.clone()))
                });
            }
            let resolve = prep.resolve.clone();
            running.override_external_effect::<crate::effects::ResolvePeerCandidate>(usize::MAX, move |eff| {
                let peer = resolve.get(&eff.candidate).and_then(|peers| peers.iter().next().copied());
                OverrideResult::handled(crate::effects::ResolvePeerCandidateResult {
                    candidate: eff.candidate.clone(),
                    origin: eff.origin,
                    peer,
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
