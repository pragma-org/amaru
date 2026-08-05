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

//! Process lifecycle: per-subcommand Tokio runtimes and main-thread signal polling.

use std::{
    error::Error,
    future::Future,
    io, process,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

use opentelemetry_sdk::metrics::SdkMeterProvider;
use parking_lot::Mutex;
use tokio::runtime::{Builder, Runtime};
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::exit::SignalState;

/// How often the main thread inspects the signal counter.
pub const SIGNAL_POLL_INTERVAL: Duration = Duration::from_millis(200);

/// Exit status used when a termination signal force-exits the process (128 + SIGINT).
pub const FORCE_EXIT_CODE: i32 = 130;

/// Grace period for draining the Tokio runtime after command and observability teardown.
pub const RUNTIME_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

static SIGNAL_STDERR_ENABLED: AtomicBool = AtomicBool::new(true);

/// Which Tokio runtime configuration a subcommand should use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeKind {
    /// Short, mostly-sync or light-async work.
    Simple,
    /// Network-bound short commands (bootstrap, snapshot download, peer fetch).
    Io,
    /// Long-running `node run`.
    Node,
}

impl RuntimeKind {
    pub fn build(self) -> io::Result<Runtime> {
        match self {
            Self::Simple => runtime_simple(),
            Self::Io => runtime_io(),
            Self::Node => runtime_node(),
        }
    }
}

pub fn set_signal_stderr_enabled(enabled: bool) {
    SIGNAL_STDERR_ENABLED.store(enabled, Ordering::SeqCst);
}

/// How the first termination signal is handled for a subcommand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirstSignal {
    /// Cancel the shutdown handle and run abort hooks; a second signal force-exits.
    SoftShutdown,
    /// Exit the process immediately (commands with no orderly shutdown ceremony).
    ImmediateExit,
}

type CmdFuture = std::pin::Pin<Box<dyn Future<Output = Result<(), Box<dyn Error>>> + 'static>>;
type CmdWork = Box<dyn FnOnce(ShutdownHandle, Option<SdkMeterProvider>) -> CmdFuture + Send>;

/// A fully resolved leaf subcommand ready to run under the process lifecycle.
///
/// Construct via [`Runnable::soft`] / [`Runnable::exit_on_signal`], then:
/// 1. [`Self::build_runtime`] — create the Tokio runtime for this command
/// 2. Set up observability while that runtime is current ([`Runtime::enter`])
/// 3. [`Self::run_on`] — bind metrics and drive the command future on that runtime
pub struct Runnable {
    runtime: RuntimeKind,
    first_signal: FirstSignal,
    work: CmdWork,
}

impl Runnable {
    /// Soft shutdown on first signal. The work closure receives a [`ShutdownHandle`] so the
    /// command can register abort hooks and await cancellation, plus any meter provider
    /// produced by observability setup (only used by long-running commands).
    pub fn soft<F, Fut>(runtime: RuntimeKind, work: F) -> Self
    where
        F: FnOnce(ShutdownHandle, Option<SdkMeterProvider>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), Box<dyn Error>>> + 'static,
    {
        Self {
            runtime,
            first_signal: FirstSignal::SoftShutdown,
            work: Box::new(move |shutdown, metrics| Box::pin(work(shutdown, metrics))),
        }
    }

    /// Exit immediately on first signal (commands with no orderly shutdown ceremony).
    ///
    /// The work closure takes no arguments: if a [`ShutdownHandle`] is needed, use [`Self::soft`].
    pub fn exit_on_signal<F, Fut>(runtime: RuntimeKind, work: F) -> Self
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), Box<dyn Error>>> + 'static,
    {
        Self {
            runtime,
            first_signal: FirstSignal::ImmediateExit,
            work: Box::new(move |_shutdown, _metrics| Box::pin(work())),
        }
    }

    pub fn build_runtime(&self) -> io::Result<Runtime> {
        self.runtime.build()
    }

    /// Drive this command on an already-built runtime (observability must already be set up).
    ///
    /// Does **not** shut down `rt`; the caller tears down observability and then the runtime.
    pub fn run_on(
        self,
        rt: &Runtime,
        signals: &SignalState,
        metrics: Option<SdkMeterProvider>,
    ) -> Result<(), Box<dyn Error>> {
        run_until_exit(rt, signals, self.first_signal, self.work, metrics)
    }

    /// Build a runtime, run the command with no meter provider, then shut the runtime down.
    ///
    /// Prefer the explicit build → observability → [`Self::run_on`] sequence in the binary.
    pub fn run(self, signals: &SignalState) -> Result<(), Box<dyn Error>> {
        let rt = self.build_runtime()?;
        let result = self.run_on(&rt, signals, None);
        rt.shutdown_timeout(RUNTIME_SHUTDOWN_TIMEOUT);
        result
    }
}

/// Cancellation token plus optional sync abort hooks invoked on first soft signal.
///
/// Abort hooks run on the **main thread** and must be cheap and non-blocking
/// (e.g. `JoinHandle::abort`). They must not await Tokio tasks.
#[derive(Clone)]
pub struct ShutdownHandle {
    cancel: CancellationToken,
    abort_hooks: Arc<Mutex<Vec<Box<dyn Fn() + Send + Sync>>>>,
}

impl ShutdownHandle {
    fn new() -> Self {
        Self { cancel: CancellationToken::new(), abort_hooks: Arc::new(Mutex::new(Vec::new())) }
    }

    pub fn token(&self) -> CancellationToken {
        self.cancel.clone()
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    /// Register a hook run when the first termination signal arrives (soft path only).
    ///
    /// If shutdown was already requested, the hook runs immediately (still intended to be
    /// non-blocking, e.g. `JoinHandle::abort`).
    pub fn register_abort(&self, hook: impl Fn() + Send + Sync + 'static) {
        let hook = Box::new(hook) as Box<dyn Fn() + Send + Sync>;
        if self.cancel.is_cancelled() {
            hook();
        }
        self.abort_hooks.lock().push(hook);
    }

    fn request_graceful(&self) {
        if !self.cancel.is_cancelled() {
            emit_signal_stderr("amaru: termination signal received — shutting down");
            warn!("termination signal received — shutting down");
            self.cancel.cancel();
        }
        let hooks = self.abort_hooks.lock();
        for hook in hooks.iter() {
            hook();
        }
    }
}

/// Multi-thread runtime for short, mostly-sync or light-async subcommands.
pub fn runtime_simple() -> io::Result<Runtime> {
    Builder::new_multi_thread().worker_threads(2).enable_all().thread_name("amaru-simple").build()
}

/// Multi-thread runtime for network-bound short commands (bootstrap, snapshot, fetch).
pub fn runtime_io() -> io::Result<Runtime> {
    Builder::new_multi_thread().worker_threads(4).enable_all().thread_name("amaru-io").build()
}

/// Multi-thread runtime for `node run`.
///
/// TODO(rkuhn): properly measure and design the Tokio runtime setup we need
/// (network vs CPU-bound vs store pipeline).
pub fn runtime_node() -> io::Result<Runtime> {
    Builder::new_multi_thread().worker_threads(4).enable_all().thread_name("amaru-node").build()
}

/// Run `work` on `rt` while the calling (main) thread polls termination signals.
///
/// The command future is driven via `Handle::block_on` on a dedicated worker thread so that
/// `Box<dyn Error>` command results do not need to be `Send` through `tokio::spawn`.
///
/// Signal policy is controlled by [`FirstSignal`].
///
/// The runtime is **not** shut down here so the caller can tear down observability on the same
/// runtime after the command completes.
pub fn run_until_exit(
    rt: &Runtime,
    signals: &SignalState,
    first_signal: FirstSignal,
    work: CmdWork,
    metrics: Option<SdkMeterProvider>,
) -> Result<(), Box<dyn Error>> {
    let shutdown = ShutdownHandle::new();
    let shutdown_work = shutdown.clone();
    let handle = rt.handle().clone();

    let (result_tx, result_rx) = mpsc::channel::<Result<(), String>>();
    let worker = thread::Builder::new()
        .name("amaru-cmd".into())
        .spawn(move || {
            let result = handle.block_on(work(shutdown_work, metrics));
            let _ = result_tx.send(result.map_err(|err| err.to_string()));
        })
        .map_err(|e| format!("failed to spawn command worker thread: {e}"))?;

    let mut graceful = false;
    let result = loop {
        match result_rx.try_recv() {
            Ok(result) => break result,
            Err(mpsc::TryRecvError::Disconnected) => {
                break Err("command worker thread ended without a result".to_string());
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }

        let n = signals.count();
        if n >= 1 {
            match first_signal {
                FirstSignal::ImmediateExit => {
                    emit_signal_stderr("amaru: termination signal received — exiting");
                    process::exit(FORCE_EXIT_CODE);
                }
                FirstSignal::SoftShutdown if n >= 2 => {
                    emit_signal_stderr("amaru: second termination signal — forcing exit");
                    process::exit(FORCE_EXIT_CODE);
                }
                FirstSignal::SoftShutdown if !graceful => {
                    graceful = true;
                    shutdown.request_graceful();
                }
                FirstSignal::SoftShutdown => {}
            }
        }

        thread::sleep(SIGNAL_POLL_INTERVAL);
    };

    match worker.join() {
        Ok(()) => {}
        Err(_) => {
            // Prefer the channel result if present; otherwise report panic.
            if result.is_ok() {
                return Err("command worker thread panicked".into());
            }
        }
    }

    result.map_err(|msg| msg.into())
}

fn emit_signal_stderr(message: &str) {
    if SIGNAL_STDERR_ENABLED.load(Ordering::SeqCst) {
        eprintln!("{message}");
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[test]
    fn first_signal_cancels_and_runs_abort_hooks() {
        let signals = SignalState::new_disconnected();
        let aborts = Arc::new(AtomicUsize::new(0));
        let aborts2 = Arc::clone(&aborts);
        let signals2 = signals.clone();

        let worker = thread::spawn(move || {
            // Simulate first signal after the command has registered its hook.
            thread::sleep(Duration::from_millis(50));
            signals2.shared_count().fetch_add(1, Ordering::SeqCst);
        });

        let result = Runnable::soft(RuntimeKind::Simple, move |shutdown, _metrics| {
            let aborts2 = aborts2;
            async move {
                shutdown.register_abort(move || {
                    aborts2.fetch_add(1, Ordering::SeqCst);
                });
                shutdown.token().cancelled().await;
                Ok(())
            }
        })
        .run(&signals);

        worker.join().expect("signal injector");
        assert!(result.is_ok());
        assert_eq!(aborts.load(Ordering::SeqCst), 1);
        assert!(signals.count() >= 1);
    }

    #[test]
    fn late_abort_registration_runs_immediately_if_already_cancelled() {
        let shutdown = ShutdownHandle::new();
        shutdown.token().cancel();
        let ran = Arc::new(AtomicUsize::new(0));
        let ran2 = Arc::clone(&ran);
        shutdown.register_abort(move || {
            ran2.fetch_add(1, Ordering::SeqCst);
        });
        assert_eq!(ran.load(Ordering::SeqCst), 1);
    }
}
