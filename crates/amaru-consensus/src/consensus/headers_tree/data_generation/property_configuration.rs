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

//! This module extends the default `proptest` configuration with a more fluent DSL for configuring properties.
//!
//! For example:
//!  ```
//!  #![proptest_config(config_begin().no_shrink().with_cases(1).end())]
//!  ```

use proptest::prelude::ProptestConfig;
use proptest::test_runner::{Config, RngSeed};

#[derive(Default, Clone)]
pub struct ProptestConfiguration {
    config: ProptestConfig,
}

impl ProptestConfiguration {
    pub fn no_shrink(mut self) -> Self {
        self.config.max_shrink_iters = 0;
        self
    }

    pub fn with_cases(mut self, n: u32) -> Self {
        self.config.cases = n;
        self
    }

    pub fn with_seed(mut self, seed: u64) -> Self {
        self.config.rng_seed = RngSeed::Fixed(seed);
        self
    }

    pub fn end(self) -> ProptestConfig {
        self.config
    }
}

pub fn config_begin() -> ProptestConfiguration {
    // Don't output files on failures by default
    ProptestConfiguration {
        config: Config {
            failure_persistence: None,
            ..Default::default()
        },
    }
}
