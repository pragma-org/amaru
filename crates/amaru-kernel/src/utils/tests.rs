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

use std::{
    panic::{AssertUnwindSafe, RefUnwindSafe, UnwindSafe, catch_unwind},
    sync::Mutex,
};

use proptest::{
    prelude::*,
    strategy::ValueTree,
    test_runner,
    test_runner::{RngSeed, TestError, TestRunner},
};
use rand::{Rng, SeedableRng, prelude::StdRng};

static SILENT_PANIC_HOOK: Mutex<()> = Mutex::new(());

/// Catch a panic without invoking the panic hook (no stderr / `RUST_BACKTRACE` dump).
///
/// The process-wide hook is replaced for the duration of `f`. Callers are serialized so concurrent
/// tests do not clobber each other's hooks.
pub fn catch_unwind_silent<F, R>(f: F) -> std::thread::Result<R>
where
    F: FnOnce() -> R + UnwindSafe,
{
    let _serialize = SILENT_PANIC_HOOK.lock().unwrap_or_else(|e| e.into_inner());
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    struct RestoreHook(Option<Box<dyn Fn(&std::panic::PanicHookInfo<'_>) + Send + Sync + 'static>>);
    impl Drop for RestoreHook {
        fn drop(&mut self) {
            if let Some(hook) = self.0.take() {
                std::panic::set_hook(hook);
            }
        }
    }
    let _restore = RestoreHook(Some(previous));
    catch_unwind(f)
}

/// Run `f` on values from `strategy` and assert that every case panics.
///
/// Use this instead of `proptest!` + `#[should_panic]`. Panics are caught with a silenced hook, and
/// shrinking is disabled (a panic is success, not a counterexample to shrink).
#[allow(clippy::panic)]
pub fn assert_strategy_always_panics<S, F>(strategy: S, mut config: ProptestConfig, f: F)
where
    S: Strategy,
    F: Fn(S::Value) + UnwindSafe + RefUnwindSafe,
{
    config.max_shrink_iters = 0;
    let mut runner = TestRunner::new(config);
    let f = &f;
    match runner.run(&strategy, |value| {
        if catch_unwind_silent(AssertUnwindSafe(|| f(value))).is_err() {
            Ok(())
        } else {
            Err(TestCaseError::fail("expected a panic"))
        }
    }) {
        Ok(()) => {}
        Err(e) => panic!("{e}"),
    }
}

/// Assert that `strategy` produces at least one value for which `f` panics.
///
/// Use this instead of `proptest!` + `#[should_panic]` when only some generated values panic.
/// Panics are caught with a silenced hook so `--no-capture` does not dump them.
pub fn assert_strategy_sometimes_panics<S, F>(strategy: S, mut config: ProptestConfig, f: F)
where
    S: Strategy,
    F: Fn(S::Value) + UnwindSafe + RefUnwindSafe,
{
    // A panic is the success we are looking for, not a counterexample to minimize.
    config.max_shrink_iters = 0;
    let f = &f;
    assert_strategy_sometimes_fails(strategy, config, |value| {
        if catch_unwind_silent(AssertUnwindSafe(|| f(value))).is_err() {
            Err(TestCaseError::fail("panicked as expected"))
        } else {
            Ok(())
        }
    });
}

/// Assert that `strategy` produces at least one value for which `test` returns `Err`.
///
/// Prefer this over `proptest!` + `#[should_panic]` when the test exists to show that a generator
/// *sometimes* hits a given case. A failing case is a `Result`, so the panic hook (and its
/// `RUST_BACKTRACE` dump) is not invoked under `--no-capture`.
#[allow(clippy::panic)]
pub fn assert_strategy_sometimes_fails<S: Strategy>(
    strategy: S,
    config: ProptestConfig,
    test: impl Fn(S::Value) -> Result<(), TestCaseError>,
) {
    let mut runner = TestRunner::new(config);
    match runner.run(&strategy, test) {
        Err(TestError::Fail(_, _)) => {}
        other => panic!("expected at least one failing case, got {other:?}"),
    }
}

/// Run a strategy with the default test runner, outside of a typical property test run.
#[allow(clippy::panic)]
pub fn run_strategy<T>(any: impl Strategy<Value = T>) -> T {
    any.new_tree(&mut TestRunner::default())
        .unwrap_or_else(|e| panic!("unable to generate random value from default test runner: {e}"))
        .current()
}

/// Run a strategy from a fixed `u64` seed so a world/simulation test can reproduce the chain.
#[allow(clippy::panic)]
pub fn run_strategy_with_seed<T>(seed: u64, any: impl Strategy<Value = T>) -> T {
    let config = test_runner::Config { rng_seed: RngSeed::Fixed(seed), ..Default::default() };
    any.new_tree(&mut TestRunner::new(config))
        .unwrap_or_else(|e| panic!("unable to generate random value from seed {seed:#x}: {e}"))
        .current()
}

/// Run a strategy with a seed provided by a random generator
/// and return the generated value, panicking if generation fails.
#[expect(clippy::unwrap_used)]
pub fn run_strategy_with_rng<T, RNG: Rng>(rng: &mut RNG, s: impl Strategy<Value = T>) -> T {
    let config = test_runner::Config { rng_seed: RngSeed::Fixed(rng.random()), ..Default::default() };
    let mut runner = TestRunner::new(config);
    s.new_tree(&mut runner).unwrap().current()
}

/// Draw a `u64` from the thread-local CSPRNG. World tests print this so a run can be replayed.
pub fn random_u64() -> u64 {
    rand::rng().random()
}

/// Get some random bytes vector of the given size.
pub fn random_bytes(size: usize) -> Vec<u8> {
    random_bytes_with_rng(&mut StdRng::from_os_rng(), size)
}

/// Get some random bytes vector of the given size and RNG
pub fn random_bytes_with_rng(rng: &mut impl Rng, size: usize) -> Vec<u8> {
    let mut buffer = vec![0; size];
    rng.fill_bytes(&mut buffer);
    buffer
}
