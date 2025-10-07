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

use crate::simulator::Args;
use pallas_crypto::hash::Hash;
use std::path::PathBuf;

/// Configuration for a single node
#[derive(Debug, Clone)]
pub struct NodeConfig {
    pub stake_distribution_file: PathBuf,
    pub consensus_context_file: PathBuf,
    pub chain_dir: PathBuf,
    pub block_tree_file: PathBuf,
    pub start_header: Hash<32>,
    pub number_of_upstream_peers: u8,
    pub number_of_downstream_peers: u8,
}

impl Default for NodeConfig {
    fn default() -> Self {
        Self {
            stake_distribution_file: PathBuf::from("./stake_distribution.json"),
            consensus_context_file: PathBuf::from("./consensus_context.json"),
            chain_dir: PathBuf::from("./chain.db/"),
            block_tree_file: PathBuf::from("./chain.json"),
            start_header: Hash::from([0; 32]),
            number_of_upstream_peers: 2,
            number_of_downstream_peers: 1,
        }
    }
}

impl NodeConfig {
    pub fn from(args: Args) -> Self {
        Self {
            stake_distribution_file: args.stake_distribution_file,
            consensus_context_file: args.consensus_context_file,
            chain_dir: args.chain_dir,
            block_tree_file: args.block_tree_file,
            start_header: args.start_header,
            number_of_upstream_peers: args.number_of_upstream_peers,
            number_of_downstream_peers: args.number_of_downstream_peers,
        }
    }

    pub fn with_number_of_upstream_peers(mut self, n: u8) -> Self {
        self.number_of_upstream_peers = n;
        self
    }

    pub fn with_number_of_downstream_peers(mut self, n: u8) -> Self {
        self.number_of_downstream_peers = n;
        self
    }
}
