// Copyright 2025 PRAGMA
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

use amaru_kernel::{IsHeader, Tip};
use amaru_observability::amaru::consensus::diffusion::FORWARD_CHAIN;
use amaru_protocols::manager::ManagerMessage;
use pure_stage::StageRef;
use tracing::{Instrument, error, trace};

use crate::{
    effects::{BaseOps, ConsensusOps},
    errors::{ProcessingFailed, ValidationFailed},
    events::BlockValidationResult,
    span::HasSpan,
};

pub const EVENT_TARGET: &str = "amaru::consensus::forward_chain";

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct ForwardChainState {
    our_tip: Tip,
    manager_stage: StageRef<ManagerMessage>,
    validation_errors: StageRef<ValidationFailed>,
    processing_errors: StageRef<ProcessingFailed>,
}

impl ForwardChainState {
    pub fn new(
        our_tip: Tip,
        manager_stage: StageRef<ManagerMessage>,
        validation_errors: StageRef<ValidationFailed>,
        processing_errors: StageRef<ProcessingFailed>,
    ) -> Self {
        Self { our_tip, manager_stage, validation_errors, processing_errors }
    }
}

/// The forward chain stage forwards the headers of validated blocks to downstream peers, via the
/// `ForwardEventEffect`. The current node tip is maintained in order to double check that the header
/// we sent out is correct
pub fn stage(
    mut state: ForwardChainState,
    msg: BlockValidationResult,
    eff: impl ConsensusOps,
) -> impl Future<Output = ForwardChainState> {
    let span = tracing::trace_span!(parent: msg.span(), FORWARD_CHAIN);
    async move {
        match msg {
            BlockValidationResult::BlockValidated { header, .. } => {
                // assert that the new tip is a direct successor of the old tip
                assert_eq!(header.block_height(), state.our_tip.block_height() + 1);
                match header.parent() {
                    Some(parent) => assert_eq!(parent, state.our_tip.hash()),
                    None => assert_eq!(state.our_tip, Tip::origin()),
                }
                state.our_tip = header.tip();
                trace!(
                    target: EVENT_TARGET,
                    point = %header.point(),
                    "diffusion.forward_chain.new_tip"
                );

                eff.base().send(&state.manager_stage, ManagerMessage::NewTip(state.our_tip)).await;
            }
            BlockValidationResult::RolledBackTo { rollback_header, .. } => {
                trace!(
                    target: EVENT_TARGET,
                    point = %rollback_header.point(),
                    "diffusion.forward_chain.rolled_back_to"
                );

                state.our_tip = rollback_header.tip();
                eff.base().send(&state.manager_stage, ManagerMessage::NewTip(state.our_tip)).await;
            }
            BlockValidationResult::BlockValidationFailed { point, .. } => {
                error!(
                    target: EVENT_TARGET,
                    point = %point,
                    "diffusion.forward_chain.block_validation.failed"
                );
            }
        }
        state
    }
    .instrument(span)
}
