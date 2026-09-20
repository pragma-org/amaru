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

use std::sync::Mutex;

use amaru_kernel::{ConsensusParameters, NULL_HASH28, PREPROD_ERA_HISTORY, PREPROD_GLOBAL_PARAMETERS};
use amaru_pure_stage::{
    DeserializerGuards, StageGraph, StageRef, simulation::SimulationRunning, stage_ref::StageStateRef,
};

use super::{ForgeBlock, ForgeBlockMsg, ForgeHeaderEffect, LeaderScheduleEffect, TakeForForgeEffect, stage};
use crate::{
    effects::ValidateHeaderEffect,
    stages::{
        select_chain::SelectChainMsg,
        test_utils::{Logs, run_simulation},
    },
};

pub const STAGE: &str = "fb-1";

pub struct TestPrep {
    pub state: ForgeBlock,
    pub rt: tokio::runtime::Runtime,
}

pub fn test_prep() -> TestPrep {
    let consensus_parameters = ConsensusParameters::new(PREPROD_GLOBAL_PARAMETERS.clone(), &PREPROD_ERA_HISTORY);
    let select_chain: StageRef<SelectChainMsg> = StageRef::named_for_tests("select_chain");
    TestPrep {
        state: ForgeBlock::new(
            select_chain,
            consensus_parameters,
            PREPROD_GLOBAL_PARAMETERS.consensus_security_param,
            NULL_HASH28,
            0,
        ),
        rt: crate::stages::test_utils::test_runtime(),
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
}

pub fn setup(
    prep: &TestPrep,
    msg: ForgeBlockMsg,
) -> (SimulationRunning, DeserializerGuards, Logs, StageStateRef<ForgeBlockMsg, ForgeBlock>) {
    setup_msgs(prep, [msg])
}

pub fn setup_msgs(
    prep: &TestPrep,
    msgs: impl IntoIterator<Item = ForgeBlockMsg>,
) -> (SimulationRunning, DeserializerGuards, Logs, StageStateRef<ForgeBlockMsg, ForgeBlock>) {
    let guards = register_guards();
    let state = prep.state.clone();
    let msgs: Vec<_> = msgs.into_iter().collect();
    let wired_slot: Mutex<Option<StageStateRef<ForgeBlockMsg, ForgeBlock>>> = Mutex::new(None);
    let (running, guards, logs) = run_simulation(
        prep.rt.handle(),
        guards,
        |mut network| {
            let stage_ref = network.stage(STAGE, stage);
            let wired = network.wire_up(stage_ref, state);
            network.preload(&wired, msgs).unwrap();
            *wired_slot.lock().expect("wired slot") = Some(wired);
            network
        },
        |_resources| {},
        |_running| {},
    );
    let wired = wired_slot.lock().expect("wired slot").take().expect("stage was wired");
    (running, guards, logs, wired)
}
