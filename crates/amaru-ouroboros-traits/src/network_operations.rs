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

use amaru_kernel::{
    Point, connection::BlockFetchClientError, consensus_events::ChainSyncEvent, peer::Peer,
};
use std::sync::Arc;

pub type ResourceNetworkOperations = Arc<dyn NetworkOperations>;

/// Network operations trait for consensus effects.
/// This trait abstracts network operations that can be used by consensus stages.
#[async_trait::async_trait]
pub trait NetworkOperations: Send + Sync {
    /// Wait for the next chain sync event from upstream peers.
    async fn next_sync(&self) -> ChainSyncEvent;

    /// Fetch a block from a specific peer at a given point.
    async fn fetch_block(
        &self,
        peer: &Peer,
        point: Point,
    ) -> Result<Vec<u8>, BlockFetchClientError>;

    /// Disconnect from a specific peer.
    async fn disconnect(&self, peer: &Peer);
}
