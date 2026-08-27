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
    /// How much of `block_size` mixed blocks carry on average, in percent; individual blocks
    /// vary around it. Single-entity scenarios ignore it and always fill whole blocks.
    pub block_fill: usize,
    pub mixed_weights: MixedWeights,
}

impl BenchScale {
    pub const ENV_VAR_VOLATILE_SIZE: &'static str = "AMARU_BENCH_VOLATILE_SIZE";

    pub const ENV_VAR_BLOCK_SIZE: &'static str = "AMARU_BENCH_BLOCK_SIZE";

    pub const ENV_VAR_BLOCK_FILL: &'static str = "AMARU_BENCH_BLOCK_FILL";

    pub fn from_env() -> Self {
        let volatile_size =
            read_env_usize(Self::ENV_VAR_VOLATILE_SIZE, MAINNET_GLOBAL_PARAMETERS.consensus_security_param as usize);

        let block_size =
            read_env_usize(Self::ENV_VAR_BLOCK_SIZE, MAINNET_DEFAULT_PROTOCOL_PARAMETERS.max_block_body_size as usize);

        let block_fill = read_env_usize(Self::ENV_VAR_BLOCK_FILL, 3);

        let mixed_weights = MixedWeights::from_env();

        Self { volatile_size, block_size, block_fill, mixed_weights }
    }

    pub fn fill_target(&self) -> usize {
        self.block_size * self.block_fill / 100
    }
}

/// Relative frequency of each entity in the mixed workload,
/// expressed as the mean number of items per collection over a 2160 block window.
///
/// The default values are sampled over blocks [13586560, 13588719].
/// Entities absent from that window default to 1
#[derive(Clone, Copy, Debug)]
pub struct MixedWeights {
    pub utxo: u64,
    pub pools: u64,
    pub accounts: u64,
    pub withdrawals: u64,
    pub committee: u64,
    pub dreps: u64,
    pub proposals: u64,
    pub votes: u64,
}

impl Default for MixedWeights {
    fn default() -> Self {
        Self {
            utxo: 47_250,
            pools: 2,
            accounts: 480,
            withdrawals: 1_320,
            committee: 1,
            dreps: 4,
            proposals: 10,
            votes: 80,
        }
    }
}

impl MixedWeights {
    pub const ENV_VAR_UTXO: &'static str = "AMARU_BENCH_MIXED_WEIGHT_UTXO";

    pub const ENV_VAR_POOLS: &'static str = "AMARU_BENCH_MIXED_WEIGHT_POOLS";

    pub const ENV_VAR_ACCOUNTS: &'static str = "AMARU_BENCH_MIXED_WEIGHT_ACCOUNTS";

    pub const ENV_VAR_WITHDRAWALS: &'static str = "AMARU_BENCH_MIXED_WEIGHT_WITHDRAWALS";

    pub const ENV_VAR_COMMITTEE: &'static str = "AMARU_BENCH_MIXED_WEIGHT_COMMITTEE";

    pub const ENV_VAR_DREPS: &'static str = "AMARU_BENCH_MIXED_WEIGHT_DREPS";

    pub const ENV_VAR_PROPOSALS: &'static str = "AMARU_BENCH_MIXED_WEIGHT_PROPOSALS";

    pub const ENV_VAR_VOTES: &'static str = "AMARU_BENCH_MIXED_WEIGHT_VOTES";

    pub fn from_env() -> Self {
        let default = Self::default();

        let weights = Self {
            utxo: read_env_u64(Self::ENV_VAR_UTXO, default.utxo),
            pools: read_env_u64(Self::ENV_VAR_POOLS, default.pools),
            accounts: read_env_u64(Self::ENV_VAR_ACCOUNTS, default.accounts),
            withdrawals: read_env_u64(Self::ENV_VAR_WITHDRAWALS, default.withdrawals),
            committee: read_env_u64(Self::ENV_VAR_COMMITTEE, default.committee),
            dreps: read_env_u64(Self::ENV_VAR_DREPS, default.dreps),
            proposals: read_env_u64(Self::ENV_VAR_PROPOSALS, default.proposals),
            votes: read_env_u64(Self::ENV_VAR_VOTES, default.votes),
        };

        assert!(weights.total() > 0, "at least one mixed weight must be non-zero");

        weights
    }

    pub fn total(&self) -> u64 {
        let Self { utxo, pools, accounts, withdrawals, committee, dreps, proposals, votes } = self;
        utxo + pools + accounts + withdrawals + committee + dreps + proposals + votes
    }

    pub fn entries(&self) -> [(&'static str, &'static str, u64); 8] {
        [
            ("utxo", Self::ENV_VAR_UTXO, self.utxo),
            ("pools", Self::ENV_VAR_POOLS, self.pools),
            ("accounts", Self::ENV_VAR_ACCOUNTS, self.accounts),
            ("withdrawals", Self::ENV_VAR_WITHDRAWALS, self.withdrawals),
            ("committee", Self::ENV_VAR_COMMITTEE, self.committee),
            ("dreps", Self::ENV_VAR_DREPS, self.dreps),
            ("proposals", Self::ENV_VAR_PROPOSALS, self.proposals),
            ("votes", Self::ENV_VAR_VOTES, self.votes),
        ]
    }
}

fn read_env_usize(name: &str, default: usize) -> usize {
    env::var(name).ok().and_then(|value| value.parse::<usize>().ok()).filter(|value| *value > 0).unwrap_or(default)
}

#[expect(clippy::panic, reason = "non-production code")]
fn read_env_u64(name: &str, default: u64) -> u64 {
    match env::var(name) {
        Err(_) => default,
        Ok(value) => value.trim().parse().unwrap_or_else(|_| panic!("{name}: invalid weight '{value}'")),
    }
}
