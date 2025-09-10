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

use crate::{ConsensusError, consensus::store::ChainStore};
use amaru_kernel::{Header, Point, RawBlock, block::ValidateBlockEvent};
use amaru_ouroboros_traits::IsHeader;
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct StoreBlock {
    store: Arc<Mutex<dyn ChainStore<Header>>>,
}

impl StoreBlock {
    pub fn new(chain_store: Arc<Mutex<dyn ChainStore<Header>>>) -> Self {
        StoreBlock { store: chain_store }
    }

    pub async fn store(&self, point: &Point, block: &RawBlock) -> Result<(), ConsensusError> {
        self.store
            .lock()
            .await
            .store_block(&point.into(), block)
            .map_err(|e| ConsensusError::StoreBlockFailed(point.clone(), e))
    }

    pub async fn handle_event(
        &self,
        event: &ValidateBlockEvent,
    ) -> Result<ValidateBlockEvent, ConsensusError> {
        match event {
            ValidateBlockEvent::Validated { header, block, .. } => {
                self.store(&header.point(), block).await?;
                Ok(event.clone())
            }
            ValidateBlockEvent::Rollback { .. } => Ok(event.clone()),
        }
    }
}
