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

use amaru_kernel::{IsHeader, Point, Tip};
use amaru_protocols::store_effects::ResourceHeaderStore;
use pure_stage::{
    Receiver, StageGraph, StageGraphRunning, StageRef,
    simulation::{SimulationBuilder, running::SimulationRunning},
};
use tokio::runtime::{Builder, Runtime};

use super::state::{StepAction, TestState};
use crate::stages::select_chain::{SelectChain, SelectChainMsg, best_tip_candidate_from_store};

/// The Harness is responsible for instantiating and running the select_chain stage.
/// It returns all the downstream outputs emitted by the stage in response to messages.
pub struct Harness {
    rt: Runtime,
    store: ResourceHeaderStore,
    running: SimulationRunning,
    stage: StageRef<SelectChainMsg>,
    output_rx: Receiver<(Tip, Point)>,
    outputs: Vec<(Tip, Point)>,
}

impl Harness {
    pub fn new(store: ResourceHeaderStore) -> Self {
        let rt = Builder::new_current_thread().enable_all().build().unwrap();
        let (running, stage, output_rx) = build_stage(store.clone());
        Self { rt, store, running, stage, output_rx, outputs: vec![] }
    }

    /// Recreate the select_chain, instantiated from the current chain store state,
    /// and send the first FextNextFrom message to trigger outputs.
    pub fn restart(&mut self) -> Vec<(Tip, Point)> {
        let (running, stage, output_rx) = build_stage(self.store.clone());
        self.running = running;
        self.stage = stage;
        self.output_rx = output_rx;
        self.run_message(SelectChainMsg::FetchNextFrom(Point::Origin))
    }

    pub fn execute_action(&mut self, action: &StepAction, state: &TestState) -> Vec<(Tip, Point)> {
        match action {
            StepAction::FetchNextFromOrigin => self.run_message(SelectChainMsg::FetchNextFrom(Point::Origin)),
            StepAction::TipFromUpstream(hash) => {
                let header = state.store().load_header(hash).unwrap();
                let parent = header
                    .parent()
                    .and_then(|parent| self.store.load_header(&parent))
                    .map(|parent| parent.point())
                    .unwrap_or(Point::Origin);
                self.run_message(SelectChainMsg::TipFromUpstream(header.tip(), parent))
            }
            StepAction::BlockValidationResult(hash, valid) => {
                let tip = state.store().load_header(hash).unwrap().tip();
                self.run_message(SelectChainMsg::BlockValidationResult(tip, *valid))
            }
            StepAction::Restart => self.restart(),
        }
    }

    fn run_message(&mut self, msg: SelectChainMsg) -> Vec<(Tip, Point)> {
        self.running.enqueue_msg(&self.stage, [msg]);
        self.running.run_until_blocked_incl_effects(self.rt.handle());
        self.collect_outputs()
    }

    fn collect_outputs(&mut self) -> Vec<(Tip, Point)> {
        let mut new_outputs = self.output_rx.drain().collect::<Vec<_>>();
        assert!(new_outputs.len() <= 1, "select_chain should emit at most one output per input, got {new_outputs:#?}");
        let mut outputs = vec![];

        if let Some((tip, parent)) = new_outputs.pop() {
            outputs.push((tip, parent));
            self.outputs.push((tip, parent));
            self.running.enqueue_msg(&self.stage, [SelectChainMsg::FetchNextFrom(tip.point())]);
            self.running.run_until_blocked_incl_effects(self.rt.handle());

            let follow_up_outputs = self.output_rx.drain().collect::<Vec<_>>();
            assert!(
                follow_up_outputs.is_empty(),
                "FetchNextFrom of the advertised tip should not emit a new output, got {follow_up_outputs:#?}"
            );
        }

        assert!(!self.running.is_terminated(), "The stage must not be terminated");
        outputs
    }
}

fn build_stage(store: ResourceHeaderStore) -> (SimulationRunning, StageRef<SelectChainMsg>, Receiver<(Tip, Point)>) {
    let mut simulation = SimulationBuilder::default().with_seed(42);
    simulation.resources().put::<ResourceHeaderStore>(store.clone());

    let (downstream, output_rx) = simulation.output::<(Tip, Point)>("downstream", 1_000_000);
    let best_candidate = best_tip_candidate_from_store(store.as_ref());
    let select_chain = SelectChain::new(downstream, best_candidate);
    let select_chain_stage = simulation.stage("select_chain", crate::stages::select_chain::stage);
    let select_chain_stage = simulation.wire_up(select_chain_stage, select_chain);

    (simulation.run(), select_chain_stage.without_state(), output_rx)
}
