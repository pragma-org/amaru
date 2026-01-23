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

use crate::manager;
use crate::manager::{Manager, ManagerMessage};
use pure_stage::Effects;
use std::time::Duration;

/// This stage allows us to simulate a responder node that will not respond right away with
/// all the data that the initiator is requesting. This is useful for testing the reconnection logic.
pub async fn slow_manager_stage(
    manager: Manager,
    msg: ManagerMessage,
    eff: Effects<ManagerMessage>,
) -> Manager {
    match msg {
        ManagerMessage::AddPeer(_) => {}
        ManagerMessage::ConnectionDied(_, _, _) => {}
        ManagerMessage::Connect(_) => {}
        ManagerMessage::Accepted(_, _) => {
            // Wait for some time before proceeding to simulate a slow manager
            tracing::info!("slow manager: waiting after accepting connection");
            eff.wait(Duration::from_secs(3)).await;
        }
        ManagerMessage::RemovePeer(_) => {}
        ManagerMessage::FetchBlocks { .. } => {}
    }
    manager::stage(manager, msg, eff).await
}
