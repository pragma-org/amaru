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

use crate::consensus::effects::NetworkOps;
use crate::consensus::effects::{BaseOps, ConsensusOps};
use crate::consensus::errors::{ProcessingFailed, ValidationFailed};
use crate::consensus::span::HasSpan;
use crate::consensus::tip::{AsHeaderTip, HeaderTip};
use amaru_kernel::IsHeader;
use amaru_kernel::consensus_events::BlockValidationResult;
use anyhow::anyhow;
use pure_stage::StageRef;
use tracing::{Level, error, info, span, trace};

pub const EVENT_TARGET: &str = "amaru::consensus::forward_chain";

type State = (
    HeaderTip,
    StageRef<ValidationFailed>,
    StageRef<ProcessingFailed>,
);

/// The forward chain stage forwards the headers of validated blocks to downstream peers, via the
/// `ForwardEventEffect`. The current node tip is maintained in order to double check that the header
/// we sent out is correct
pub async fn stage(state: State, msg: BlockValidationResult, eff: impl ConsensusOps) -> State {
    let span = span!(parent: msg.span(), Level::TRACE, "stage.forward_chain");
    let _entered = span.enter();

    let (mut our_tip, validation_errors, processing_errors) = state;
    match msg {
        BlockValidationResult::BlockValidated { peer, header, .. } => {
            // assert that the new tip is a direct successor of the old tip
            // assert_eq!(header.block_height(), our_tip.block_height() + 1);
            // match header.parent() {
            //     Some(parent) => assert_eq!(parent, our_tip.hash()),
            //     None => assert_eq!(our_tip, HeaderTip::new(Point::Origin, 0)),
            // }
            our_tip = header.as_header_tip();
            trace!(
                target: EVENT_TARGET,
                tip = %header.point(),
                "tip_changed"
            );

            if let Err(e) = eff
                .network()
                .send_forward_event(peer.clone(), header.clone())
                .await
            {
                error!(
                    target: EVENT_TARGET,
                    %e,
                    "failed to send forward event"
                );
                eff.base()
                    .send(&processing_errors, ProcessingFailed::new(&peer, anyhow!(e)))
                    .await
            }
        }
        BlockValidationResult::RolledBackTo {
            peer,
            rollback_header,
            ..
        } => {
            info!(
                target: EVENT_TARGET,
                point = %rollback_header.point(),
                "rolled_back_to"
            );

            our_tip = rollback_header.as_header_tip();
            if let Err(e) = eff
                .network()
                .send_backward_event(peer.clone(), rollback_header.as_header_tip())
                .await
            {
                error!(
                    target: EVENT_TARGET,
                    %e,
                    "failed to send backward event"
                );
                eff.base()
                    .send(&processing_errors, ProcessingFailed::new(&peer, anyhow!(e)))
                    .await
            }
        }
        BlockValidationResult::BlockValidationFailed { point, .. } => {
            error!(
                target: EVENT_TARGET,
                slot = %point.slot_or_default(),
                hash = %point.hash(),
                "block validation failed"
            );
        }
    }
    (our_tip, validation_errors, processing_errors)
}
