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

#[derive(Debug, Clone, Default)]
pub struct SimulateConfig {
    pub number_of_tests: u32,
    pub seed: u64,
    pub number_of_nodes: u8,
    pub disable_shrinking: bool,
}

impl SimulateConfig {
    pub fn with_number_of_tests(mut self, n: u32) -> Self {
        self.number_of_tests = n;
        self
    }

    pub fn with_seed(mut self, seed: u64) -> Self {
        self.seed = seed;
        self
    }

    pub fn with_number_of_nodes(mut self, n: u8) -> Self {
        self.number_of_nodes = n;
        self
    }

    pub fn disable_shrinking(mut self) -> Self {
        self.disable_shrinking = true;
        self
    }
}
