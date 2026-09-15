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

#![expect(clippy::unwrap_used, reason = "non-production bench code")]

use common::{
    fixture::{empty_block_at, seed_and_build_state},
    mock_store::roll_forward,
    scale::EpochBenchScale,
};
use divan::Bencher;

mod common;

pub fn main() {
    print_configuration();
    divan::main();
}

fn print_configuration() {
    let scale = EpochBenchScale::from_env();
    eprintln!("epoch_transition bench configuration");
    let fmt = |var| format!("(env.{}={})", var, std::env::var(var).ok().as_deref().unwrap_or("<unset>"));
    eprintln!("├─ pools={} {}", scale.pools, fmt(EpochBenchScale::ENV_VAR_POOLS));
    eprintln!("├─ utxos={} {}", scale.utxos, fmt(EpochBenchScale::ENV_VAR_UTXOS));
    eprintln!("╰─ accounts={} {}\n", scale.accounts, fmt(EpochBenchScale::ENV_VAR_ACCOUNTS));
}

/// Measures the full epoch transition: rewards computation (background thread), transition body,
/// and stable flush + snapshot. Setup seeds a RocksDB at the configured scale, builds a State,
/// and drives it to the block that triggers rewards computation. The timed portion is the single
/// roll_forward at the epoch boundary that joins the thread and completes the transition.
#[divan::bench]
fn bench_epoch_transition(bencher: Bencher<'_, '_>) {
    let scale = EpochBenchScale::from_env();
    bencher
        .with_inputs(|| seed_and_build_state(&scale))
        .bench_values(|(mut state, boundary_slot)| {
            roll_forward(&mut state, &empty_block_at(boundary_slot));
        });
}
