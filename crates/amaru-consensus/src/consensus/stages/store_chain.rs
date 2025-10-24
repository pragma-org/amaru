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

use crate::consensus::effects::{BaseOps, ConsensusOps};
use crate::consensus::errors::ProcessingFailed;
use crate::consensus::span::HasSpan;
use amaru_kernel::consensus_events::BlockValidationResult;
use pure_stage::StageRef;
use tracing::{Level, span};

type State = (
    // downstream stage to which we simply pass along events
    StageRef<BlockValidationResult>,
    // where to send processing errors
    StageRef<ProcessingFailed>,
);

/// The `store_chain` stage is responsible for persisting the state of the _best chain_ in the `ChainStore`
///
/// * When the chain is extended, it simply records the new block's point as an extension of the best chain
/// * When the chain is rolled back, it discards the tip of the chain until before the rollback point
///
/// The reason this is a stage and not an `Effect` is because it's
/// critically important we maintain a consistent best chain in the
/// store and do not forward any invalid information should there be a
/// failure or we detect an inconsistency.
pub async fn stage(state: State, msg: BlockValidationResult, eff: impl ConsensusOps) -> State {
    let span = span!(parent: msg.span(), Level::TRACE, "stage.store_chain");
    let _entered = span.enter();

    let (downstream, processing_errors) = state;
    match msg {
        BlockValidationResult::BlockValidated { .. } => eff.base().send(&downstream, msg).await,
        BlockValidationResult::RolledBackTo { .. } => eff.base().send(&downstream, msg).await,
        BlockValidationResult::BlockValidationFailed { .. } => {
            eff.base().send(&downstream, msg).await
        }
    }
    (downstream, processing_errors)
}
