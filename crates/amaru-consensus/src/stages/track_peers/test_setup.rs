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

use std::sync::Arc;

use amaru_kernel::{BlockHeader, ConsensusParameters, EraHistory, HeaderHash, NetworkName, Tip, make_header};
use amaru_ouroboros::ConnectionId;
use amaru_ouroboros_traits::{
    MockCanValidateBlocks, PoolSummaries, WriteChainStore, in_memory_chain_store::InMemoryChainStore,
};
use amaru_protocols::{
    chainsync::{self, InitiatorMessage},
    manager::ManagerMessage,
    store_effects::{HasHeaderEffect, LoadHeaderEffect, LoadTipEffect, ResourceHeaderStore, StoreHeaderEffect},
};
use amaru_pure_stage::{
    DeserializerGuards, Effect, StageGraph, StageRef, TraceMatch,
    simulation::{SimulationRunning, running::OverrideResult},
    trace_buffer::TraceEntry,
};
use opentelemetry::Context;
use tokio::runtime::{Builder, Handle, Runtime};

use super::*;
use crate::{
    effects::{
        ResourceBlockValidation, ResourceConsensusParameters, ResourceEraHistory, ResourceHasStakePools,
        ResourcePoolSummaries, TipEffect, ValidateHeaderEffect, VolatileTipEffect,
    },
    stages::{
        peer_selection::PeerSelectionMsg,
        test_utils::{Logs, run_simulation},
    },
};

pub fn build_store(headers: &[BlockHeader]) -> Arc<InMemoryChainStore> {
    let store = Arc::new(InMemoryChainStore::new());
    for header in headers {
        store.store_header(header).unwrap();
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
}

impl TestPrep {
    pub fn rt_handle(&self) -> Handle {
        self.rt.handle().clone()
    }
}

/// Creates basic state, runtime, handler, conn_id, and three properly linked headers for tests.
pub fn test_prep() -> TestPrep {
    test_prep_with_security_param(10_000_000)
}

/// Creates a `TestPrep` with a configurable consensus security parameter (for testing defer logic).
pub fn test_prep_with_security_param(security_param: u64) -> TestPrep {
    let state = TrackPeers::new(
        EraHistory::default(),
        StageRef::named_for_tests("peer_selection"),
        StageRef::named_for_tests("downstream"),
        security_param,
    );
    let rt = Builder::new_current_thread().build().unwrap();
    let handler = StageRef::<InitiatorMessage>::named_for_tests("handler");
    let conn_id = ConnectionId::initial();
    let h1 = make_block_header(1, 1, None);
    let h2 = make_block_header(2, 2, Some(h1.hash()));
    let h3 = make_block_header(3, 3, Some(h2.hash()));
    TestPrep { state, rt, handler, conn_id, headers: [h1, h2, h3] }
}

pub fn make_block_header(block_number: u64, slot: u64, parent: Option<HeaderHash>) -> BlockHeader {
    BlockHeader::from(make_header(block_number, slot, parent))
}

pub fn te_validate_header(at_stage: &str, header: BlockHeader) -> TraceEntry {
    TraceEntry::suspend(Effect::external(at_stage, Box::new(ValidateHeaderEffect::new(&header, Context::new()))))
}

pub fn te_load_tip(at_stage: &str, hash: HeaderHash) -> TraceEntry {
    TraceEntry::suspend(Effect::external(at_stage, Box::new(LoadTipEffect::new(hash))))
}

pub fn te_has_header(at_stage: &str, hash: HeaderHash) -> TraceEntry {
    TraceEntry::suspend(Effect::external(at_stage, Box::new(HasHeaderEffect::new(hash))))
}

pub fn te_store_header(at_stage: &str, header: BlockHeader) -> TraceEntry {
    TraceEntry::suspend(Effect::external(at_stage, Box::new(StoreHeaderEffect::new(header))))
}

pub fn tm_store_header(at_stage: &str) -> TraceMatch<'_> {
    TraceMatch::Property(
        Box::new(
            move |e| matches!(e, TraceEntry::Suspend(Effect::External { at_stage: at, effect }) if at.as_str() == at_stage && effect.is::<StoreHeaderEffect>()),
        ),
        format!("store_header at {}", at_stage),
    )
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
        amaru_pure_stage::register_data_deserializer::<Tip>().boxed(),
        amaru_pure_stage::register_data_deserializer::<(Tip, Point)>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<LoadHeaderEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<LoadTipEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<HasHeaderEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<StoreHeaderEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<ValidateHeaderEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<TipEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<VolatileTipEffect>().boxed(),
    ]
}

pub fn setup(
    rt: &Handle,
    state: TrackPeers,
    msg: TrackPeersMsg,
    store: Arc<InMemoryChainStore>,
) -> (SimulationRunning, DeserializerGuards, Logs) {
    setup_base(rt, state, msg, store, |running| {
        running.override_external_effect::<ValidateHeaderEffect>(usize::MAX, |_| OverrideResult::handled(Ok(())));
    })
}

/// Setup variant that forces a specific ledger-applied tip (used to test the defer path).
pub fn setup_with_ledger_tip(
    rt: &Handle,
    state: TrackPeers,
    msg: TrackPeersMsg,
    store: Arc<InMemoryChainStore>,
    ledger_tip: Tip,
) -> (SimulationRunning, DeserializerGuards, Logs) {
    setup_base(rt, state, msg, store, |running| {
        running.override_external_effect::<ValidateHeaderEffect>(usize::MAX, |_| OverrideResult::handled(Ok(())));
        // Force the ledger height returned by VolatileTipEffect / TipEffect so we can control defer decisions.
        running.override_external_effect::<VolatileTipEffect>(usize::MAX, {
            move |_| OverrideResult::handled(ledger_tip)
        });
        running.override_external_effect::<TipEffect>(usize::MAX, move |_| OverrideResult::handled(ledger_tip));
    })
}

pub(crate) fn setup_base(
    rt: &Handle,
    state: TrackPeers,
    msg: TrackPeersMsg,
    store: Arc<InMemoryChainStore>,
    overrides: impl FnOnce(&mut SimulationRunning),
) -> (SimulationRunning, DeserializerGuards, Logs) {
    run_simulation(
        rt,
        register_guards(),
        |network| {
            let tp = network.stage("tp", stage);
            let tp = network.wire_up(tp, state);
            network.preload(&tp, [msg]).unwrap();
        },
        |resources| {
            resources.put::<ResourceHeaderStore>(store.clone());
            let block_validation = Arc::new(MockCanValidateBlocks);
            resources.put::<ResourceBlockValidation>(block_validation.clone());
            resources.put::<ResourceHasStakePools>(block_validation);
            let era_history = NetworkName::Preprod.as_era_history().expect("preprod era for tests").clone();
            let global = NetworkName::Preprod.as_global_parameters().expect("preprod global for tests").clone();
            let cp = Arc::new(ConsensusParameters::new(global, &era_history, Default::default()));
            resources.put::<ResourceConsensusParameters>(cp);
            resources.put::<ResourceEraHistory>(era_history);
            resources.put::<ResourcePoolSummaries>(Arc::new(PoolSummaries::default()));
        },
        overrides,
    )
}
