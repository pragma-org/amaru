// Copyright 2026 PRAGMA
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

use std::{collections::BTreeSet, net::SocketAddr};

use async_trait::async_trait;

use crate::StoreError;

/// This trait provides peer data sourced from the ledger.
#[async_trait]
pub trait HasPeersData: Send + Sync {
    /// Return the relay addresses registered by pools.
    async fn registered_relay_socket_addrs(&self) -> Result<BTreeSet<SocketAddr>, StoreError>;
}
