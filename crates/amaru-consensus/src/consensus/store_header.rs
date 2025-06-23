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
use std::{fmt, sync::Arc};
use tokio::sync::Mutex;

use super::DecodedChainSyncEvent;

#[derive(serde::Serialize, serde::Deserialize)]
pub struct StoreHeader {
    #[serde(skip, default = "default_store")]
    pub store: Arc<Mutex<dyn ChainStore<Header>>>,
}
fn default_store() -> Arc<Mutex<dyn ChainStore<Header>>> {
    Arc::new(Mutex::new(super::store::FakeStore::default()))
}

impl PartialEq for StoreHeader {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl fmt::Debug for StoreHeader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StoreHeader")
            .field("store", &"Arc<Mutex<dyn ChainStore<Header>>>")
            .finish()
    }
}

impl StoreHeader {
    pub fn new(chain_store: Arc<Mutex<dyn ChainStore<Header>>>) -> Self {
        StoreHeader { store: chain_store }
    }

    pub async fn store(&self, point: &Point, header: &Header) -> Result<(), ConsensusError> {
        // FIXME: we should check the header is not already known and _invalid_ to
        // prevent a peer from flooding use with duplicate crappy headers
        self.store
            .lock()
            .await
            .store_header(&header.hash(), header)
            .map_err(|e| ConsensusError::StoreHeaderFailed(point.clone(), e))
    }

    pub async fn handle_event(
        &self,
        event: DecodedChainSyncEvent,
    ) -> Result<DecodedChainSyncEvent, ConsensusError> {
        match event {
            DecodedChainSyncEvent::RollForward {
                ref point,
                ref header,
                ..
            } => {
                self.store(point, header).await?;
                Ok(event)
            }
            DecodedChainSyncEvent::Rollback { .. } => Ok(event),
        }
    }
}
