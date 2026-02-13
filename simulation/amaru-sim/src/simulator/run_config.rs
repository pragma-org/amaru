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
use amaru_kernel::Peer;
use pure_stage::simulation::RandStdRng;
use rand::prelude::StdRng;
use rand::{Rng, SeedableRng};
use std::path::{Path, PathBuf};

/// Configuration for a simulation run
#[derive(Debug, Clone)]
pub struct RunConfig {
    pub number_of_tests: u32,
    pub seed: u64,
    pub number_of_upstream_peers: u8,
    pub number_of_downstream_peers: u8,
    pub generated_chain_depth: usize,
    pub enable_shrinking: bool,
    pub persist_on_success: bool,
    pub persist_directory: PathBuf,
}

impl Default for RunConfig {
    fn default() -> Self {
        Self {
            number_of_tests: 50,
            seed: rand::rng().random::<u64>(),
            number_of_upstream_peers: 1,
            number_of_downstream_peers: 1,
            generated_chain_depth: 4,
            enable_shrinking: false,
            persist_on_success: true,
            persist_directory: Path::new("test-data").to_path_buf(),
        }
    }
}

impl RunConfig {
    pub fn from(args: Args) -> Self {
        let default = Self::default();
        Self {
            number_of_tests: args.number_of_tests,
            seed: args.seed.unwrap_or(default.seed),
            number_of_upstream_peers: args.number_of_upstream_peers,
            number_of_downstream_peers: args.number_of_downstream_peers,
            generated_chain_depth: args.generated_chain_depth,
            enable_shrinking: args.enable_shrinking,
            persist_on_success: args.persist_on_success,
            persist_directory: Path::new(&args.persist_directory).to_path_buf(),
        }
    }
    pub fn with_number_of_tests(mut self, n: u32) -> Self {
        self.number_of_tests = n;
        self
    }

    pub fn with_seed(mut self, seed: u64) -> Self {
        self.seed = seed;
        self
    }

    pub fn with_number_of_nodes(mut self, n: u8) -> Self {
        self.number_of_upstream_peers = n;
        self
    }

    pub fn disable_shrinking(mut self) -> Self {
        self.enable_shrinking = false;
        self
    }

    pub fn persist_on_success(mut self, persist: bool) -> Self {
        self.persist_on_success = persist;
        self
    }

    pub fn persist_directory(mut self, directory: PathBuf) -> Self {
        self.persist_directory = directory;
        self
    }

    pub fn upstream_peers(&self) -> Vec<Peer> {
        (1..=self.number_of_upstream_peers)
            .map(|i| Peer::new(&format!("127.0.0.1:300{i}")))
            .collect()
    }

    pub fn downstream_peers(&self) -> Vec<Peer> {
        (1..=self.number_of_downstream_peers)
            .map(|i| Peer::new(&format!("127.0.0.1:400{i}")))
            .collect()
    }

    pub fn rng(&self) -> RandStdRng {
        RandStdRng(StdRng::seed_from_u64(self.seed))
    }
}
