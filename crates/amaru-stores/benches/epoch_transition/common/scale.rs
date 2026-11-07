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

#[derive(Clone, Copy, Debug)]
pub struct EpochBenchScale {
    pub pools: usize,
    pub utxos: usize,
    pub accounts: usize,
}

impl EpochBenchScale {
    pub const ENV_VAR_POOLS: &'static str = "AMARU_BENCH_POOLS";
    pub const ENV_VAR_UTXOS: &'static str = "AMARU_BENCH_UTXOS";
    pub const ENV_VAR_ACCOUNTS: &'static str = "AMARU_BENCH_ACCOUNTS";

    pub fn from_env() -> Self {
        Self {
            pools: read_env_usize(Self::ENV_VAR_POOLS, 100),
            utxos: read_env_usize(Self::ENV_VAR_UTXOS, 10_000),
            accounts: read_env_usize(Self::ENV_VAR_ACCOUNTS, 500),
        }
    }
}

fn read_env_usize(name: &str, default: usize) -> usize {
    env::var(name).ok().and_then(|v| v.parse::<usize>().ok()).filter(|v| *v > 0).unwrap_or(default)
}
