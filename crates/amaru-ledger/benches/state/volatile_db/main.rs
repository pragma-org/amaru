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

#![expect(clippy::unwrap_used, reason = "non-production code")]

use std::{
    collections::BTreeMap,
    sync::{Arc, LazyLock, RwLock},
};

use common::{scale::BenchScale, scenario::Scenario};
use divan::Bencher;

mod benches;
mod common;

static MEMORY_USAGE: LazyLock<Arc<RwLock<BTreeMap<Scenario, u64>>>> =
    LazyLock::new(|| Arc::new(RwLock::new(BTreeMap::new())));

pub fn main() {
    print_configuration();
    divan::main();
    print_memory_usage();
}

fn print_configuration() {
    use benches::SwitchToFork;

    eprintln!("configuration");

    let scale = BenchScale::from_env();
    let fork_point = SwitchToFork::new(Scenario::Mixed).fork_point();

    let format_env_var = |var| format!("(env.{}={})", var, std::env::var(var).ok().as_deref().unwrap_or("<unset>"));

    eprintln!("├─ volatile_size={} {}", scale.volatile_size, format_env_var(BenchScale::ENV_VAR_VOLATILE_SIZE));
    eprintln!("├─ block_size={} {}", scale.block_size, format_env_var(BenchScale::ENV_VAR_BLOCK_SIZE));
    eprintln!("╰─ fork_point={} {}\n", fork_point, format_env_var(SwitchToFork::ENV_VAR_FORK_POINT));
}

fn print_memory_usage() {
    eprintln!("memory usage (volatile footprint)");
    let mut scenarios = Scenario::round_robin().iter().chain(std::iter::once(&Scenario::Mixed)).collect::<Vec<_>>();
    let last_index = scenarios.len() - 1;
    scenarios.sort_by_key(|scenario| scenario.name());
    scenarios.iter().enumerate().for_each(|(ix, scenario)| {
        eprintln!(
            "{}─ {}={}MB",
            if ix == last_index { "╰" } else { "├" },
            scenario.name(),
            MEMORY_USAGE.read().unwrap().get(scenario).copied().unwrap_or_default()
        );
    })
}

#[divan::bench(args = [
    benches::RollForward::new(Scenario::Accounts),
    benches::RollForward::new(Scenario::Committee),
    benches::RollForward::new(Scenario::DReps),
    benches::RollForward::new(Scenario::Mixed),
    benches::RollForward::new(Scenario::Pools),
    benches::RollForward::new(Scenario::Proposals),
    benches::RollForward::new(Scenario::Utxo),
    benches::RollForward::new(Scenario::Votes),
    benches::RollForward::new(Scenario::Withdrawals),
])]
fn bench_roll_forward(bencher: Bencher<'_, '_>, bench: benches::RollForward) {
    let rss = bench.run(bencher);
    let mut memory_usage = MEMORY_USAGE.write().unwrap();
    memory_usage.insert(bench.scenario, rss);
}

#[divan::bench(args = [
    benches::SwitchToFork::new(Scenario::Accounts),
    benches::SwitchToFork::new(Scenario::Committee),
    benches::SwitchToFork::new(Scenario::DReps),
    benches::SwitchToFork::new(Scenario::Mixed),
    benches::SwitchToFork::new(Scenario::Pools),
    benches::SwitchToFork::new(Scenario::Proposals),
    benches::SwitchToFork::new(Scenario::Utxo),
    benches::SwitchToFork::new(Scenario::Votes),
    benches::SwitchToFork::new(Scenario::Withdrawals),
])]
fn bench_switch_to_fork(bencher: Bencher<'_, '_>, bench: benches::SwitchToFork) {
    bench.run(bencher);
}

#[divan::bench(args = [
    benches::HydrateContext::new(Scenario::Accounts),
    benches::HydrateContext::new(Scenario::Committee),
    benches::HydrateContext::new(Scenario::DReps),
    benches::HydrateContext::new(Scenario::Mixed),
    benches::HydrateContext::new(Scenario::Pools),
    benches::HydrateContext::new(Scenario::Proposals),
    benches::HydrateContext::new(Scenario::Utxo),
    benches::HydrateContext::new(Scenario::Votes),
    benches::HydrateContext::new(Scenario::Withdrawals),
])]
fn bench_hydrate_context(bencher: Bencher<'_, '_>, bench: benches::HydrateContext) {
    bench.run(bencher);
}
