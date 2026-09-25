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

use std::sync::{Arc, Mutex};

use amaru_kernel::{
    ConsensusParameters, KesPeriod, NULL_HASH28, PREPROD_ERA_HISTORY, PREPROD_GLOBAL_PARAMETERS, ProtocolVersion,
};
use amaru_ouroboros_traits::{ForgingCredentials, Nonces, in_memory_chain_store::InMemoryChainStore};
use amaru_protocols::store_effects::ResourceHeaderStore;
use amaru_pure_stage::{
    DeserializerGuards, StageGraph, StageRef,
    simulation::{SimulationRunning, running::OverrideResult},
    stage_ref::StageStateRef,
};

pub use super::TestCredentials;
use super::{
    ForgeBlock, ForgeBlockMsg, ForgeHeaderEffect, LeaderScheduleEffect, ResourceForgingCredentials, TakeForForgeEffect,
    schedule::EpochSchedule, stage, test_vrf_key,
};
use crate::{
    effects::ValidateHeaderEffect,
    stages::{
        select_chain::SelectChainMsg,
        test_utils::{Logs, SimulationRunMode, run_simulation_with},
    },
};

pub const STAGE: &str = "fb-1";

pub fn vrf_key() -> amaru_ouroboros::vrf::SecretKey {
    test_vrf_key()
}

pub struct TestPrep {
    pub state: ForgeBlock,
    pub rt: tokio::runtime::Runtime,
    pub store: Arc<InMemoryChainStore>,
    pub credentials: Option<Arc<TestCredentials>>,
}

pub fn test_prep() -> TestPrep {
    let consensus_parameters = ConsensusParameters::new(PREPROD_GLOBAL_PARAMETERS.clone(), &PREPROD_ERA_HISTORY);
    let ocert_start_period = KesPeriod::from(0);
    let credentials = TestCredentials::for_test_keys(ocert_start_period, consensus_parameters.max_kes_evolutions());
    let select_chain: StageRef<SelectChainMsg> = StageRef::named_for_tests("select_chain");
    TestPrep {
        state: ForgeBlock::new(
            select_chain,
            consensus_parameters,
            PREPROD_GLOBAL_PARAMETERS.system_start,
            PREPROD_GLOBAL_PARAMETERS.consensus_security_param,
            NULL_HASH28,
            ocert_start_period,
            ProtocolVersion::new(11, 0),
        ),
        rt: crate::stages::test_utils::test_runtime(),
        store: Arc::new(InMemoryChainStore::new()),
        credentials: Some(Arc::new(credentials)),
    }
}

pub fn register_guards() -> DeserializerGuards {
    vec![
        amaru_pure_stage::register_data_deserializer::<ForgeBlock>().boxed(),
        amaru_pure_stage::register_data_deserializer::<ForgeBlockMsg>().boxed(),
        amaru_pure_stage::register_data_deserializer::<SelectChainMsg>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<LeaderScheduleEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<ForgeHeaderEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<TakeForForgeEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<ValidateHeaderEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<amaru_protocols::store_effects::StoreValidatedHeaderEffect>()
            .boxed(),
        amaru_pure_stage::register_effect_deserializer::<amaru_protocols::store_effects::StoreBlockEffect>().boxed(),
    ]
    .into_iter()
    .chain(amaru_protocols::store_effects::register_deserializers())
    .collect()
}

pub fn setup(
    prep: &TestPrep,
    msg: ForgeBlockMsg,
) -> (SimulationRunning, DeserializerGuards, Logs, StageStateRef<ForgeBlockMsg, ForgeBlock>) {
    setup_msgs(prep, [msg])
}

/// Drive `msg` until the stage next sleeps on a timer, without firing that timer.
pub fn setup_until_sleeping(
    prep: &TestPrep,
    msg: ForgeBlockMsg,
) -> (SimulationRunning, DeserializerGuards, Logs, StageStateRef<ForgeBlockMsg, ForgeBlock>) {
    setup_with(prep, [msg], SimulationRunMode::UntilSleeping)
}

pub fn setup_msgs(
    prep: &TestPrep,
    msgs: impl IntoIterator<Item = ForgeBlockMsg>,
) -> (SimulationRunning, DeserializerGuards, Logs, StageStateRef<ForgeBlockMsg, ForgeBlock>) {
    setup_with(prep, msgs, SimulationRunMode::UntilBlocked)
}

fn setup_with(
    prep: &TestPrep,
    msgs: impl IntoIterator<Item = ForgeBlockMsg>,
    mode: SimulationRunMode,
) -> (SimulationRunning, DeserializerGuards, Logs, StageStateRef<ForgeBlockMsg, ForgeBlock>) {
    let guards = register_guards();
    let state = prep.state.clone();
    let msgs: Vec<_> = msgs.into_iter().collect();
    let wired_slot: Mutex<Option<StageStateRef<ForgeBlockMsg, ForgeBlock>>> = Mutex::new(None);
    let (running, guards, logs) = run_simulation_with(
        prep.rt.handle(),
        guards,
        |mut network| {
            let stage_ref = network.stage(STAGE, stage);
            let wired = network.wire_up(stage_ref, state);
            network.preload(&wired, msgs).unwrap();
            *wired_slot.lock().expect("wired slot") = Some(wired);
            network
        },
        |resources| {
            resources.put::<ResourceHeaderStore>(prep.store.clone());
            let credentials = prep.credentials.clone().map(|credentials| credentials as Arc<dyn ForgingCredentials>);
            resources.put::<ResourceForgingCredentials>(credentials);
        },
        |running| {
            running.override_external_effect::<LeaderScheduleEffect>(usize::MAX, |effect| {
                OverrideResult::handled(EpochSchedule::empty(effect.epoch, effect.nonce))
            });
            running.override_external_effect::<ValidateHeaderEffect>(usize::MAX, |_effect| {
                OverrideResult::handled(Ok(Nonces::for_tests()))
            });
        },
        mode,
    );
    let wired = wired_slot.lock().expect("wired slot").take().expect("stage was wired");
    (running, guards, logs, wired)
}
