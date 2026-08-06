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

pub mod chain_realign;
pub mod ledger_reset;
pub mod peer_snapshot;
pub mod stages;
pub mod submit_api;

pub use chain_realign::{ClearValidity, realign_chain_store_to};
pub use ledger_reset::reset_ledger_to_epoch;

#[cfg(any(test, feature = "test-utils"))]
pub mod tests;

pub const DEFAULT_PEER_REMOVAL_COOLDOWN_SECS: u64 = 600; // 10 minutes
pub const DEFAULT_UPSTREAM_PEERS: usize = 3;
pub const DEFAULT_DOWNSTREAM_PEERS: usize = 10;
