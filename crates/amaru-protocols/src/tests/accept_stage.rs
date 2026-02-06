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

use crate::manager::ManagerMessage;
use crate::mempool_effects::MemoryPool;
use crate::network_effects::{Network, NetworkOps};
use crate::tests::configuration::get_tx_ids;
use amaru_ouroboros_traits::TxSubmissionMempool;
use pure_stage::{Effects, StageRef};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct PullAccept;

#[derive(Debug, Deserialize, Serialize)]
pub struct AcceptState {
    manager_stage: StageRef<ManagerMessage>,
    #[serde(skip)]
    notify: Arc<Notify>,
}

impl PartialEq for AcceptState {
    fn eq(&self, other: &Self) -> bool {
        self.manager_stage == other.manager_stage
    }
}

impl Eq for AcceptState {}

impl AcceptState {
    pub(super) fn new(manager_stage: StageRef<ManagerMessage>, notify: Arc<Notify>) -> Self {
        Self {
            manager_stage,
            notify,
        }
    }
}

/// Create a stage that accepts incoming connections and notifies the manager
/// about them. This can not be implemented using contramap because we need to
/// create a sender for that stage and this is not possible with an adapted stage.
pub async fn accept_stage(
    state: AcceptState,
    _msg: PullAccept,
    eff: Effects<PullAccept>,
) -> AcceptState {
    // Terminate if the mempool already has all expected transactions
    let mempool = MemoryPool::new(eff.clone());
    let expected_tx_ids = get_tx_ids();
    let txs = mempool.get_txs_for_ids(expected_tx_ids.as_slice());
    if txs.len() == expected_tx_ids.len() {
        tracing::info!("all txs retrieved, done");
        state.notify.notify_waiters();
    } else {
        tracing::info!(
            "still missing txs {}, continuing",
            expected_tx_ids.len() - txs.len()
        );
    }

    match Network::new(&eff).accept().await {
        Ok((peer, connection_id)) => {
            eff.send(
                &state.manager_stage,
                ManagerMessage::Accepted(peer, connection_id),
            )
            .await;
        }
        Err(err) => {
            tracing::error!(?err, "failed to accept a connection");
            return eff.terminate().await;
        }
    }
    eff.schedule_after(PullAccept, Duration::from_millis(100))
        .await;
    state
}
