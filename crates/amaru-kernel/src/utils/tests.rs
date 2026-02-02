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

use proptest::{
    prelude::*,
    strategy::ValueTree,
    test_runner,
    test_runner::{RngSeed, TestRunner},
};
use rand::{SeedableRng, prelude::StdRng};

/// Run a strategy with the default test runner, outside of a typical property test run.
#[allow(clippy::panic)]
pub fn run_strategy<T>(any: impl Strategy<Value = T>) -> T {
    any.new_tree(&mut TestRunner::default())
        .unwrap_or_else(|e| panic!("unable to generate random value from default test runner: {e}"))
        .current()
}

/// Run a strategy with a seed provided by a random generator
/// and return the generated value, panicking if generation fails.
#[expect(clippy::unwrap_used)]
pub fn run_strategy_with_rng<T, RNG: Rng>(rng: &mut RNG, s: impl Strategy<Value = T>) -> T {
    let config = test_runner::Config {
        rng_seed: RngSeed::Fixed(rng.random()),
        ..Default::default()
    };
    let mut runner = TestRunner::new(config);
    s.new_tree(&mut runner).unwrap().current()
}

/// Get some random bytes vector of the given size.
pub fn random_bytes(size: usize) -> Vec<u8> {
    let mut rng = StdRng::from_os_rng();
    let mut buffer = vec![0; size];
    rng.fill_bytes(&mut buffer);
    buffer
}
