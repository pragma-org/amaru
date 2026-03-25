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

use std::{
    fmt,
    path::{Path, PathBuf},
};

use amaru_kernel::Peer;
use pure_stage::simulation::RandStdRng;
use rand::{Rng, SeedableRng, prelude::StdRng};

use crate::simulator::Args;

/// Configuration for a simulation run
#[derive(Debug, Clone)]
pub struct RunConfig {
    pub number_of_tests: u32,
    pub seed: u64,
    pub number_of_upstream_peers: u8,
    pub number_of_downstream_peers: u8,
    pub generated_chain_depth: usize,
    pub number_of_transactions: usize,
    pub enable_shrinking: bool,
    pub persist_on_success: bool,
    pub test_data_dir: PathBuf,
}

impl Default for RunConfig {
    fn default() -> Self {
        Self {
            number_of_tests: 50,
            seed: rand::rng().random::<u64>(),
            number_of_upstream_peers: 1,
            number_of_downstream_peers: 1,
            generated_chain_depth: 4,
            number_of_transactions: 10,
            enable_shrinking: true,
            persist_on_success: false,
            test_data_dir: Path::new("test-data").to_path_buf(),
        }
    }
}

impl fmt::Display for RunConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Simulation configuration:")?;
        writeln!(f, "  number_of_tests:           {}", self.number_of_tests)?;
        writeln!(f, "  seed:                      {}", self.seed)?;
        writeln!(f, "  number_of_upstream_peers:   {}", self.number_of_upstream_peers)?;
        writeln!(f, "  number_of_downstream_peers: {}", self.number_of_downstream_peers)?;
        writeln!(f, "  generated_chain_depth:      {}", self.generated_chain_depth)?;
        writeln!(f, "  enable_shrinking:           {}", self.enable_shrinking)?;
        writeln!(f, "  persist_on_success:         {}", self.persist_on_success)?;
        write!(f, "  test_data_dir:              {}", self.test_data_dir.display())
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
            number_of_transactions: args.number_of_transactions,
            enable_shrinking: args.enable_shrinking,
            persist_on_success: args.persist_on_success,
            test_data_dir: Path::new(&args.test_data_dir).to_path_buf(),
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
        self.test_data_dir = directory;
        self
    }

    pub fn upstream_peers(&self) -> Vec<Peer> {
        (1..=self.number_of_upstream_peers as u16).map(|i| Peer::new(&format!("127.0.0.1:{}", 30000 + i))).collect()
    }

    pub fn downstream_peers(&self) -> Vec<Peer> {
        (1..=self.number_of_downstream_peers as u16).map(|i| Peer::new(&format!("127.0.0.1:{}", 40000 + i))).collect()
    }

    pub fn rng(&self) -> RandStdRng {
        RandStdRng(StdRng::seed_from_u64(self.seed))
    }
}
