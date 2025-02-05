// Copyright 2024 PRAGMA
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

use crate::consensus::header::{point_hash, ConwayHeader};
use crate::consensus::store::ChainStore;
use amaru_ledger::BlockValidationResult;
use gasket::framework::*;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{error, info, warn};

pub type UpstreamPort = gasket::messaging::InputPort<BlockValidationResult>;

/// Forwarding stage of the consensus where blocks are stored and made
/// available to downstream peers.
///
/// TODO: currently does nothing, should store block, update chain state, and
/// forward new chain downstream

#[derive(Stage)]
#[stage(
    name = "consensus.forward",
    unit = "BlockValidationResult",
    worker = "Worker"
)]
pub struct ForwardStage {
    pub store: Arc<Mutex<dyn ChainStore<ConwayHeader>>>,
    pub upstream: UpstreamPort,
}

impl ForwardStage {
    pub fn new(store: Arc<Mutex<dyn ChainStore<ConwayHeader>>>) -> Self {
        Self {
            store,
            upstream: Default::default(),
        }
    }
}

pub struct Worker {}

#[async_trait::async_trait(?Send)]
impl gasket::framework::Worker<ForwardStage> for Worker {
    async fn bootstrap(_stage: &ForwardStage) -> Result<Self, WorkerError> {
        Ok(Self {})
    }

    async fn schedule(
        &mut self,
        stage: &mut ForwardStage,
    ) -> Result<WorkSchedule<BlockValidationResult>, WorkerError> {
        let unit = stage.upstream.recv().await.or_panic()?;

        Ok(WorkSchedule::Unit(unit.payload))
    }

    async fn execute(
        &mut self,
        unit: &BlockValidationResult,
        _stage: &mut ForwardStage,
    ) -> Result<(), WorkerError> {
        match unit {
            BlockValidationResult::BlockValidated(point) => {
                info!(name: "block_validated", target: "amaru::consensus::chain_forward", slot = ?point.slot_or_default(), hash = ?point_hash(point).to_string());
                Ok(())
            }
            BlockValidationResult::BlockForwardStorageFailed(point) => {
                error!(name: "storage_failed", target: "amaru::consensus::chain_forward", slot = ?point.slot_or_default(), hash = ?point_hash(point).to_string());
                Err(WorkerError::Panic)
            }
            BlockValidationResult::InvalidRollbackPoint(point) => {
                warn!(name: "invalid_rollback_point", target: "amaru::consensus::chain_forward", slot = ?point.slot_or_default(), hash = ?point_hash(point).to_string());
                Ok(())
            }
            BlockValidationResult::RolledBackTo(point) => {
                info!(name:  "rolled_back_to", target: "amaru::consensus::chain_forward", slot = ?point.slot_or_default(), hash = ?point_hash(point).to_string());
                Ok(())
            }
        }
    }
}
