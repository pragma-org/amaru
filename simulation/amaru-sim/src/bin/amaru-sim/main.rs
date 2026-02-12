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

use amaru_sim::simulator::{Args, run::run_old};
use clap::Parser;
use tracing_subscriber::EnvFilter;

fn main() {
    let args = Args::parse();

    // Initialize tracing subscriber with environment variable filter
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    // It might be necessary to run the simulation with a larger stack with RUST_MIN_STACK=16777216 (16MB)
    // because of the deep recursion used when generating data for large chains.
    run_old(args);
}
