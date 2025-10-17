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

use amaru_consensus::consensus::headers_tree::data_generation::Ratio;
use clap::Parser;
use std::str::FromStr;

#[derive(Debug, Parser, Clone)]
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

    /// Ratio of rollbacks in the generated chain for a given peer
    #[arg(long, default_value = "1/2", value_parser = Ratio::from_str)]
    pub generated_chain_rollback_ratio: Ratio,

    /// Ratio of branches generated from a central chain that can be explored by peers during input generation
    #[arg(long, default_value = "1/2", value_parser = Ratio::from_str)]
    pub generated_chain_branching_ratio: Ratio,

    #[arg(long)]
    pub disable_shrinking: bool,

    /// Seed for simulation testing.
    #[arg(long)]
    pub seed: Option<u64>,

    /// Persist pure-stage's effect trace aka schedule even if the test passes.
    #[arg(long)]
    pub persist_on_success: bool,
}
