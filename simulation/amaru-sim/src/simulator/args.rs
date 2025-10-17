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

use amaru_kernel::HeaderHash;
use clap::Parser;
use pallas_crypto::hash::Hash;
use std::path::PathBuf;

#[derive(Debug, Parser, Clone)]
#[clap(name = "Amaru Simulator")]
#[clap(bin_name = "amaru-sim")]
#[clap(author, version, about, long_about = None)]
pub struct Args {
    /// Path of JSON-formatted stake distribution file.
    #[arg(long, default_value = "./stake_distribution.json")]
    pub stake_distribution_file: PathBuf,

    /// Path of JSON-formatted consensus context file.
    #[arg(long, default_value = "./consensus_context.json")]
    pub consensus_context_file: PathBuf,

    /// Path of the chain on-disk storage.
    #[arg(long, default_value = "./chain.db/")]
    pub chain_dir: PathBuf,

    /// Generated "block tree" file in JSON
    #[arg(long, default_value = "./chain.json")]
    pub block_tree_file: PathBuf,

    /// Starting point for the (simulated) chain.
    /// Default to genesis hash, eg. all-zero hash.
    #[arg(long, default_value_t = Hash::from([0; 32]))]
    pub start_header: HeaderHash,

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

    #[arg(long)]
    pub disable_shrinking: bool,

    /// Seed for simulation testing.
    #[arg(long)]
    pub seed: Option<u64>,

    /// Persist pure-stage's effect trace aka schedule even if the test passes.
    #[arg(long)]
    pub persist_on_success: bool,
}
