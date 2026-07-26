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

use std::env;

use amaru_kernel::{MAINNET_DEFAULT_PROTOCOL_PARAMETERS, MAINNET_GLOBAL_PARAMETERS};

#[derive(Clone, Copy, Debug)]
pub struct BenchScale {
    pub volatile_size: usize,
    pub block_size: usize,
}

impl BenchScale {
    pub const ENV_VAR_VOLATILE_SIZE: &'static str = "AMARU_BENCH_VOLATILE_SIZE";

    pub const ENV_VAR_BLOCK_SIZE: &'static str = "AMARU_BENCH_BLOCK_SIZE";

    pub fn from_env() -> Self {
        let volatile_size =
            read_env_usize(Self::ENV_VAR_VOLATILE_SIZE, MAINNET_GLOBAL_PARAMETERS.consensus_security_param as usize);

        let block_size =
            read_env_usize(Self::ENV_VAR_BLOCK_SIZE, MAINNET_DEFAULT_PROTOCOL_PARAMETERS.max_block_body_size as usize);

        Self { volatile_size, block_size }
    }
}

fn read_env_usize(name: &str, default: usize) -> usize {
    env::var(name).ok().and_then(|value| value.parse::<usize>().ok()).filter(|value| *value > 0).unwrap_or(default)
}
