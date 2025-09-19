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

use crate::consensus::effects::store_effects::Store;
use crate::consensus::store::StoreOps;
use crate::consensus::{
    errors::ProcessingFailed, events::ValidateBlockEvent, span::adopt_current_span,
};
use amaru_ouroboros_traits::IsHeader;
use pure_stage::{Effects, StageRef};
use tracing::Level;
use tracing::instrument;

type State = (StageRef<ValidateBlockEvent>, StageRef<ProcessingFailed>);

/// This stages stores a full block from a peer
/// It then sends the full block to the downstream stage for validation and storage.
#[instrument(
    level = Level::TRACE,
    skip_all,
    name = "stage.store_block",
)]
pub async fn stage(
    (downstream, errors): State,
    msg: ValidateBlockEvent,
    mut eff: Effects<ValidateBlockEvent>,
) -> State {
    adopt_current_span(&msg);
    match msg {
        ValidateBlockEvent::Validated {
            ref header,
            ref block,
            ref peer,
            ..
        } => match Store(&mut eff)
            .store_block(peer, &header.point(), block)
            .await
        {
            Ok(_) => eff.send(&downstream, msg).await,
            Err(e) => eff.send(&errors, e).await,
        },
        ValidateBlockEvent::Rollback { .. } => eff.send(&downstream, msg).await,
    }
    (downstream, errors)
}
