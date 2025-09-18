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

use crate::consensus::effects::network_effects::{ForwardEvent, ForwardEventEffect};
use crate::consensus::errors::{ProcessingFailed, ValidationFailed};
use crate::consensus::events::BlockValidationResult;
use crate::consensus::span::adopt_current_span;
use crate::consensus::tip::{AsHeaderTip, HeaderTip};
use amaru_kernel::Point;
use amaru_ouroboros_traits::IsHeader;
use anyhow::anyhow;
use pure_stage::{Effects, StageRef};
use tracing::{Level, error, info, instrument, trace};

pub const EVENT_TARGET: &str = "amaru::consensus::forward_chain";

type State = (
    HeaderTip,
    StageRef<ValidationFailed>,
    StageRef<ProcessingFailed>,
);

/// The forward chain stage forwards the headers of validated blocks to downstream peers, via the
/// `ForwardEventEffect`. The current node tip is maintained in order to double check that the header
/// we sent out is correct
#[instrument(
    level = Level::TRACE,
    skip_all,
    name = "stage.forward_chain",
)]
pub async fn stage(
    state: State,
    msg: BlockValidationResult,
    eff: Effects<BlockValidationResult>,
) -> State {
    adopt_current_span(&msg);
    let (mut our_tip, validation_errors, processing_errors) = state;
    match msg {
        BlockValidationResult::BlockValidated { peer, header, .. } => {
            // assert that the new tip is a direct successor of the old tip
            assert_eq!(header.block_height(), our_tip.block_height() + 1);
            match header.parent() {
                Some(parent) => assert_eq!(parent, our_tip.hash()),
                None => assert_eq!(our_tip, HeaderTip::new(Point::Origin, 0)),
            }
            our_tip = header.as_header_tip();
            trace!(
                target: EVENT_TARGET,
                tip = %header.point(),
                "tip_changed"
            );

            if let Err(e) = eff
                .external(ForwardEventEffect::new(
                    &peer,
                    ForwardEvent::Forward(header.clone()),
                ))
                .await
            {
                error!(
                    target: EVENT_TARGET,
                    %e,
                    "failed to send forward event"
                );
                eff.send(&processing_errors, ProcessingFailed::new(&peer, anyhow!(e)))
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
                .external(ForwardEventEffect::new(
                    &peer,
                    ForwardEvent::Backward(rollback_header.as_header_tip()),
                ))
                .await
            {
                error!(
                    target: EVENT_TARGET,
                    %e,
                    "failed to send backward event"
                );
                eff.send(&processing_errors, ProcessingFailed::new(&peer, anyhow!(e)))
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
