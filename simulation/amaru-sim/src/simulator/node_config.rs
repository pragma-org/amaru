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
use amaru_consensus::consensus::headers_tree::data_generation::Ratio;

/// Configuration for a single node
#[derive(Debug, Clone)]
pub struct NodeConfig {
    pub number_of_upstream_peers: u8,
    pub number_of_downstream_peers: u8,
    pub generated_chain_depth: u64,
    pub generated_chain_rollback_ratio: Ratio,
    pub generated_chain_branching_ratio: Ratio,
}

impl Default for NodeConfig {
    fn default() -> Self {
        Self {
            number_of_upstream_peers: 2,
            number_of_downstream_peers: 1,
            generated_chain_depth: 10,
            generated_chain_rollback_ratio: Ratio(1, 2),
            generated_chain_branching_ratio: Ratio(1, 2),
        }
    }
}

impl NodeConfig {
    pub fn from(args: Args) -> Self {
        Self {
            number_of_upstream_peers: args.number_of_upstream_peers,
            number_of_downstream_peers: args.number_of_downstream_peers,
            generated_chain_depth: args.generated_chain_depth,
            generated_chain_rollback_ratio: args.generated_chain_rollback_ratio,
            generated_chain_branching_ratio: args.generated_chain_branching_ratio,
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

    pub fn with_generated_chain_depth(mut self, depth: u64) -> Self {
        self.generated_chain_depth = depth;
        self
    }

    pub fn with_generated_chain_rollback_ratio(mut self, ratio: Ratio) -> Self {
        self.generated_chain_rollback_ratio = ratio;
        self
    }

    pub fn with_generated_chain_branching_ratio(mut self, ratio: Ratio) -> Self {
        self.generated_chain_branching_ratio = ratio;
        self
    }
}
