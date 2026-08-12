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

//! Product-binary entry point for process/build gauges.
//!
//! Implementation lives in [`amaru_node::system_metrics`]; this module only
//! supplies the product package version and git identity.

use std::sync::Arc;

use amaru_metrics::Meter;
use amaru_node::{
    BuildIdentity, record_block_replay_ready as record_block_replay_ready_node,
    track_system_metrics as track_system_metrics_node,
};
use tokio::task::JoinHandle;

use crate::version;

/// Record product build identity and start the process metrics poller.
pub fn track_system_metrics(meter: Arc<Meter>) -> Result<Option<JoinHandle<()>>, Box<dyn std::error::Error>> {
    track_system_metrics_node(
        meter,
        BuildIdentity {
            version: version::package_version(),
            revision: version::git_commit_hash_short().unwrap_or("unknown"),
            dirty: version::git_dirty().unwrap_or(false),
            os: version::target_os(),
            arch: version::target_arch(),
        },
    )
}

/// Expose cardano-node's replay-progress metric once Amaru's stage graph is running.
pub fn record_block_replay_ready(meter: &Meter) {
    record_block_replay_ready_node(meter);
}
