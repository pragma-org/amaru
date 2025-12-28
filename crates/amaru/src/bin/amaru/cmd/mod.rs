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

use amaru_kernel::network::NetworkName;
use pallas_network::facades::PeerClient;
use std::fmt;

pub(crate) mod bootstrap;
pub(crate) mod convert_ledger_state;
pub(crate) mod dump_chain_db;
pub(crate) mod fetch_chain_headers;
pub(crate) mod import_headers;
pub(crate) mod import_ledger_state;
pub(crate) mod import_nonces;
pub(crate) mod migrate_chain_db;
pub(crate) mod run;

/// Establish a connection to another peer. The connection are discriminated by network types.
pub(crate) async fn connect_to_peer(
    peer_address: &str,
    network: &NetworkName,
) -> Result<PeerClient, pallas_network::facades::Error> {
    PeerClient::connect(peer_address, network.to_network_magic().as_u64()).await
        .inspect_err(|reason| tracing::error!(peer = %peer_address, reason = %reason, "failed to connect to peer"))
}

#[derive(Debug)]
pub enum WorkerError {
    Recv,
    Panic,
    Restart,
    Retry,
}

impl fmt::Display for WorkerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WorkerError::Recv => write!(f, "error receiving work unit through input port"),
            WorkerError::Panic => write!(f, "operation panic, stage should stop"),
            WorkerError::Restart => write!(f, "operation requires a restart of the stage"),
            WorkerError::Retry => write!(f, "operation should be retried"),
        }
    }
}

impl std::error::Error for WorkerError {}
