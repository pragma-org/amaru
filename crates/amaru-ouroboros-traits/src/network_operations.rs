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

use amaru_kernel::is_header::HeaderTip;
use amaru_kernel::tx_submission_events::TxServerRequest;
use amaru_kernel::{
    BlockHeader, IsHeader, Point, TxClientReply,
    connection::ClientConnectionError,
    consensus_events::{ChainSyncEvent, Tracked},
    peer::Peer,
};
use serde::{Deserialize, Serialize};
use std::fmt::Display;
use std::sync::Arc;

pub type ResourceNetworkOperations = Arc<dyn NetworkOperations>;

/// Network operations trait for consensus effects.
/// This trait abstracts network operations that can be used by consensus stages.
#[async_trait::async_trait]
pub trait NetworkOperations: Send + Sync {
    /// Wait for the next chain sync event from upstream peers.
    /// This can either be one of the `ChainSyncEvent` variants or a notification that
    /// we caught up with this peer's chain.
    async fn next_sync(&self) -> Tracked<ChainSyncEvent>;

    /// Wait for the next tx request event from upstream peers.
    async fn next_tx_request(&self) -> Result<TxServerRequest, ClientConnectionError>;

    /// Send a client reply to a specific upstream peer.
    async fn send_tx_reply(&self, reply: TxClientReply) -> Result<(), ClientConnectionError>;

    /// Fetch a block from a specific peer at a given point.
    async fn fetch_block(
        &self,
        peer: &Peer,
        point: Point,
    ) -> Result<Vec<u8>, ClientConnectionError>;

    /// Disconnect from a specific upstream peer.
    async fn disconnect(&self, peer: &Peer);

    /// Wait for the next tx reply event from downstream peers.
    async fn next_tx_reply(&self) -> Result<TxClientReply, ClientConnectionError>;

    /// Send a server request to a specific downstream peer.
    async fn send_tx_request(&self, request: TxServerRequest) -> Result<(), ClientConnectionError>;

    async fn send(&self, event: ForwardEvent) -> anyhow::Result<()>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ForwardEvent {
    Forward(BlockHeader),
    Backward(HeaderTip),
}

impl ForwardEvent {
    pub fn point(&self) -> Point {
        match self {
            ForwardEvent::Forward(header) => header.point(),
            ForwardEvent::Backward(tip) => tip.point(),
        }
    }
}

impl Display for ForwardEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ForwardEvent::Forward(header) => write!(f, "Forward({})", header.point()),
            ForwardEvent::Backward(tip) => write!(f, "Backward({})", tip),
        }
    }
}
