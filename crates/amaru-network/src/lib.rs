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

use amaru_kernel::network::NetworkName;
use pallas_network::facades::PeerClient;

pub mod chain_sync_client;
pub mod point;
pub mod session;

/// Establish a connection to another peer. The connection are discriminated by network types.
pub async fn connect_to_peer(
    peer_address: &str,
    network: &NetworkName,
) -> Result<PeerClient, pallas_network::facades::Error> {
    PeerClient::connect(peer_address, network.to_network_magic() as u64).await
        .inspect_err(|reason| tracing::error!(peer = %peer_address, reason = %reason, "failed to connect to peer"))
}
