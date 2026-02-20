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

use crate::manager::{ManagerConfig, ManagerMessage};
use crate::network_effects::{AcceptError, Network, NetworkOps};
use pure_stage::{Effects, StageRef};
use std::net::SocketAddr;

/// Create a stage that repeatedly accepts incoming connections and notifies the manager about them.
pub async fn stage(state: AcceptState, _msg: PullAccept, eff: Effects<PullAccept>) -> AcceptState {
    match Network::new(&eff).accept(state.listener_addr).await {
        Ok((peer, connection_id)) => {
            eff.send(
                &state.manager_stage,
                ManagerMessage::Accepted(peer, connection_id),
            )
            .await;
        }
        Err(AcceptError::ConnectionAborted) => {
            tracing::debug!("failed to accept a connection: connection aborted");
        }
        Err(AcceptError::Other(err)) => {
            tracing::error!(?err, "failed to accept a connection");
            return eff.terminate().await;
        }
    }
    eff.schedule_after(PullAccept, state.manager_config.accept_interval)
        .await;
    state
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct PullAccept;

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct AcceptState {
    manager_stage: StageRef<ManagerMessage>,
    manager_config: ManagerConfig,
    listener_addr: SocketAddr,
}

impl AcceptState {
    pub fn new(
        manager_stage: StageRef<ManagerMessage>,
        manager_config: ManagerConfig,
        listener_addr: SocketAddr,
    ) -> Self {
        Self {
            manager_stage,
            manager_config,
            listener_addr,
        }
    }
}

pub fn register_deserializers() -> pure_stage::DeserializerGuards {
    vec![pure_stage::register_data_deserializer::<AcceptState>().boxed()]
}
