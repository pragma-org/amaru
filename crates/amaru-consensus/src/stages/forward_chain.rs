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

use crate::{
    effects::{BaseOps, ConsensusOps},
    errors::{ProcessingFailed, ValidationFailed},
    events::BlockValidationResult,
    span::HasSpan,
};
use amaru_kernel::{IsHeader, Tip};
use amaru_protocols::manager::ManagerMessage;
use pure_stage::StageRef;
use tracing::{Instrument, error, trace};

pub const EVENT_TARGET: &str = "amaru::consensus::forward_chain";

type State = (
    Tip,
    StageRef<ManagerMessage>,
    StageRef<ValidationFailed>,
    StageRef<ProcessingFailed>,
);

/// The forward chain stage forwards the headers of validated blocks to downstream peers, via the
/// `ForwardEventEffect`. The current node tip is maintained in order to double check that the header
/// we sent out is correct
pub fn stage(
    state: State,
    msg: BlockValidationResult,
    eff: impl ConsensusOps,
) -> impl Future<Output = State> {
    let span = tracing::trace_span!(parent: msg.span(), "diffusion.forward_chain");
    async move {
        let (mut our_tip, manager, validation_errors, processing_errors) = state;
        match msg {
            BlockValidationResult::BlockValidated { header, .. } => {
                // assert that the new tip is a direct successor of the old tip
                assert_eq!(header.block_height(), our_tip.block_height() + 1);
                match header.parent() {
                    Some(parent) => assert_eq!(parent, our_tip.hash()),
                    None => assert_eq!(our_tip, Tip::origin()),
                }
                our_tip = header.tip();
                trace!(
                    target: EVENT_TARGET,
                    point = %header.point(),
                    "diffusion.forward_chain.new_tip"
                );

                eff.base()
                    .send(&manager, ManagerMessage::NewTip(our_tip))
                    .await;
            }
            BlockValidationResult::RolledBackTo {
                rollback_header, ..
            } => {
                trace!(
                    target: EVENT_TARGET,
                    point = %rollback_header.point(),
                    "diffusion.forward_chain.rolled_back_to"
                );

                our_tip = rollback_header.tip();
                eff.base()
                    .send(&manager, ManagerMessage::NewTip(our_tip))
                    .await;
            }
            BlockValidationResult::BlockValidationFailed { point, .. } => {
                error!(
                    target: EVENT_TARGET,
                    point = %point,
                    "diffusion.forward_chain.block_validation.failed"
                );
            }
        }
        (our_tip, manager, validation_errors, processing_errors)
    }
    .instrument(span)
}
