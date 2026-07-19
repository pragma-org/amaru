// Copyright 2024 PRAGMA
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

//! Process termination signals, independent of the Tokio runtime.
//!
//! Handlers only increment an atomic counter (async-signal-safe). The main thread
//! polls the counter and drives graceful shutdown / forced exit.

use std::{
    io,
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
};

/// Shared signal counter installed by [`install_termination_signals`].
///
/// Values:
/// - `0` — no termination signal yet
/// - `1` — request orderly shutdown
/// - `≥2` — force-exit the process
#[derive(Clone, Debug)]
pub struct SignalState {
    count: Arc<AtomicU8>,
}

impl SignalState {
    /// Create a state that is not wired to OS signals (for tests).
    pub fn new_disconnected() -> Self {
        Self { count: Arc::new(AtomicU8::new(0)) }
    }

    /// Current number of termination signals observed (saturating at `u8::MAX`).
    pub fn count(&self) -> u8 {
        self.count.load(Ordering::SeqCst)
    }

    /// Shared atomic for tests or advanced wiring.
    pub fn shared_count(&self) -> Arc<AtomicU8> {
        Arc::clone(&self.count)
    }
}

/// Install SIGINT and SIGTERM handlers that increment a shared counter.
///
/// Uses only `signal_hook` facilities that also work on Windows CRT signals.
/// Handlers perform no logging or allocation beyond atomic arithmetic.
pub fn install_termination_signals() -> io::Result<SignalState> {
    let count = Arc::new(AtomicU8::new(0));

    for sig in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
        let count = Arc::clone(&count);
        // SAFETY: the handler only performs an atomic fetch_add, which is
        // async-signal-safe. No locks, allocation, or logging.
        unsafe {
            signal_hook::low_level::register(sig, move || {
                let old = count.fetch_add(1, Ordering::SeqCst);
                if old == 254 {
                    count.store(254, Ordering::SeqCst);
                }
            })?;
        }
    }

    Ok(SignalState { count })
}
