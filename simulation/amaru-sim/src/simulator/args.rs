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

use crate::simulator::{NodeConfig, SimulateConfig};
use clap::Parser;
use serde::{Deserialize, Serialize};

#[derive(Debug, Parser, Clone, Serialize, Deserialize)]
#[clap(name = "Amaru Simulator")]
#[clap(bin_name = "amaru-sim")]
#[clap(author, version, about, long_about = None)]
pub struct Args {
    /// Number of tests to run in simulation
    #[arg(long, default_value = "50")]
    pub number_of_tests: u32,

    /// Number of nodes in simulation.
    #[arg(long, default_value = "1")]
    pub number_of_nodes: u8,

    /// Number of upstream peers to simulate
    #[arg(long, default_value = "2")]
    pub number_of_upstream_peers: u8,

    /// Number of downstream peers to simulate
    #[arg(long, default_value = "1")]
    pub number_of_downstream_peers: u8,

    /// Maximum depth of the generated chain for a given peer
    #[arg(long, default_value = "10")]
    pub generated_chain_depth: u64,

    #[arg(long)]
    pub disable_shrinking: bool,

    /// Seed for simulation testing.
    #[arg(long)]
    pub seed: Option<u64>,

    /// Persist generated data and pure-stage traces even if the test passes.
    #[arg(long)]
    pub persist_on_success: bool,

    /// Directory where test data must be persisted
    #[arg(long, default_value = "../../target/tests")]
    pub persist_directory: String,
}

impl Args {
    pub fn from_configs(simulate_config: &SimulateConfig, node_config: &NodeConfig) -> Self {
        Self {
            number_of_tests: simulate_config.number_of_tests,
            number_of_nodes: simulate_config.number_of_nodes,
            number_of_upstream_peers: node_config.number_of_upstream_peers,
            number_of_downstream_peers: node_config.number_of_downstream_peers,
            generated_chain_depth: node_config.generated_chain_depth,
            disable_shrinking: simulate_config.disable_shrinking,
            seed: Some(simulate_config.seed),
            persist_on_success: simulate_config.persist_on_success,
            persist_directory: simulate_config.persist_directory.to_string_lossy().into(),
        }
    }
}
