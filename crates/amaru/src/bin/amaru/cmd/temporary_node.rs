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

//! Helpers for CLI commands that briefly run a node against bootstrapped stores.

use std::{path::PathBuf, sync::Arc, time::SystemTime};

use amaru_consensus::consensus_mode::classify;
use amaru_kernel::{NetworkName, Point};
use amaru_metrics::Meter;
use amaru_node::{NodeBuilder, NodeRunning, path_is_populated};
use amaru_pure_stage::Instant;
use anyhow::{Context, Result, ensure};
use tokio::runtime::Handle;

/// Configure a node with existing stores and an ephemeral loopback listener.
pub(super) fn builder(
    network: NetworkName,
    ledger: PathBuf,
    chain: PathBuf,
    peers: Vec<String>,
    meter: Meter,
) -> Result<NodeBuilder> {
    ensure!(network.as_era_history().is_some(), "a built-in network is required");
    ensure!(
        path_is_populated(&ledger)? && path_is_populated(&chain)?,
        "existing bootstrapped ledger and chain stores are required; run amaru node bootstrap first"
    );
    Ok(NodeBuilder::new(network)?
        .ledger_dir(ledger)
        .chain_dir(chain)
        .peers(peers)
        .listen_ephemeral_localhost()
        .bind_outbound(false)
        .target_downstream_peers(0)
        .meter(Arc::new(meter)))
}

/// Start the node on the current runtime, offloading blocking startup work.
pub(super) async fn start(builder: NodeBuilder) -> Result<NodeRunning> {
    let runtime = Handle::current();
    Ok(tokio::task::spawn_blocking(move || runtime.block_on(builder.start(&runtime)))
        .await
        .context("node startup task failed")??)
}

/// Check whether the tip's slot start is within the consensus live window of the wall clock.
pub(super) fn live(network: NetworkName, point: Point) -> Result<bool> {
    let era = network.as_era_history().context("missing era history")?;
    let start = network.as_global_parameters().context("missing global parameters")?.system_start();
    let elapsed = SystemTime::now().duration_since(start).context("clock precedes network genesis")?;
    Ok(point != Point::Origin
        && classify(point.slot(), Instant::at_offset(elapsed, std::time::Duration::ZERO), era).is_live())
}
