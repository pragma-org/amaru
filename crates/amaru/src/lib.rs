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

pub mod point;

/// Sync pipeline
///
/// The sync pipeline is responsible for fetching blocks from the upstream node and
/// applying them to the local chain.
pub mod stages;

/// Generic exit handler
pub mod exit;

pub const SNAPSHOTS_DIR: &str = "snapshots";

pub fn snapshots_dir(network: NetworkName) -> String {
    format!("{}/{}", SNAPSHOTS_DIR, network)
}
