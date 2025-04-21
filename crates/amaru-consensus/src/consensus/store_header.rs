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

use crate::{consensus::store::ChainStore, ConsensusError};
use amaru_kernel::{Header, Point};
use amaru_ouroboros_traits::IsHeader;
use std::sync::Arc;
use tokio::sync::Mutex;

use super::PullEvent;

pub struct StoreHeader {
    store: Arc<Mutex<dyn ChainStore<Header>>>,
}

impl StoreHeader {
    pub fn new(chain_store: Arc<Mutex<dyn ChainStore<Header>>>) -> Self {
        StoreHeader { store: chain_store }
    }

    pub async fn store(&self, point: &Point, header: &Header) -> Result<(), ConsensusError> {
        self.store
            .lock()
            .await
            .store_header(&header.hash(), header)
            .map_err(|e| ConsensusError::StoreHeaderFailed(point.clone(), e))
    }

    pub async fn handle_event(&self, event: &PullEvent) -> Result<PullEvent, ConsensusError> {
        match event {
            PullEvent::RollForward(_peer, point, header, _span) => {
                self.store(point, header).await?;
                Ok(event.clone())
            }
            PullEvent::Rollback(_peer, _point) => Ok(event.clone()),
        }
    }
}

pub async fn store_header(
    store: Arc<Mutex<dyn ChainStore<Header>>>,
    point: &Point,
    header: &Header,
) -> Result<(), ConsensusError> {
    let store_header = StoreHeader { store };
    store_header.store(point, header).await
}
