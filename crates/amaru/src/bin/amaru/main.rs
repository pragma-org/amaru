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

use std::{error::Error, process::ExitCode, time::Duration};

use amaru::{
    exit::install_termination_signals,
    lifecycle::{RUNTIME_SHUTDOWN_TIMEOUT, set_signal_stderr_enabled},
    observability::{Color, LocalTelemetry, ObservabilityHints, OpenTelemetryHandle, setup_observability},
    panic::panic_handler,
    version,
};
#[cfg(feature = "counting-allocator")]
use amaru_kernel::utils::memory::{AllocationSnapshot, CountingAllocator};
use amaru_observability::error;
use amaru_tui as tui;
use anyhow::anyhow;

#[cfg(feature = "counting-allocator")]
mod allocator_samples;
mod cli;
mod cmd;
mod pid;

#[cfg(any(
    all(feature = "jemalloc", feature = "mimalloc"),
    all(feature = "jemalloc", feature = "dhat-heap"),
    all(feature = "mimalloc", feature = "dhat-heap"),
))]
compile_error!(
    "allocator features are mutually exclusive: enable at most one of `jemalloc`, `mimalloc`, or `dhat-heap`"
);

#[cfg(all(not(target_family = "windows"), feature = "jemalloc", feature = "counting-allocator"))]
#[global_allocator]
static GLOBAL: CountingAllocator<tikv_jemallocator::Jemalloc> = CountingAllocator::new(tikv_jemallocator::Jemalloc);

#[cfg(all(not(target_family = "windows"), feature = "jemalloc", not(feature = "counting-allocator")))]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

#[cfg(all(not(target_family = "windows"), feature = "mimalloc", feature = "counting-allocator"))]
#[global_allocator]
static GLOBAL: CountingAllocator<mimalloc::MiMalloc> = CountingAllocator::new(mimalloc::MiMalloc);

#[cfg(all(not(target_family = "windows"), feature = "mimalloc", not(feature = "counting-allocator")))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[cfg(all(feature = "dhat-heap", feature = "counting-allocator"))]
#[global_allocator]
static GLOBAL: CountingAllocator<dhat::Alloc> = CountingAllocator::new(dhat::Alloc);

#[cfg(all(feature = "dhat-heap", not(feature = "counting-allocator")))]
#[global_allocator]
static GLOBAL: dhat::Alloc = dhat::Alloc;

#[cfg(all(
    feature = "counting-allocator",
    not(feature = "jemalloc"),
    not(feature = "mimalloc"),
    not(feature = "dhat-heap"),
))]
#[global_allocator]
static GLOBAL: CountingAllocator<std::alloc::System> = CountingAllocator::new(std::alloc::System);

fn main() -> ExitCode {
    panic_handler();

    match try_main() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            error!(
                cli::ERROR,
                description = err.to_string(),
                cause = @err.source().as_ref().map(|e| tracing::field::display(e.to_string())),
            );
            ExitCode::FAILURE
        }
    }
}

fn try_main() -> Result<(), Box<dyn Error>> {
    #[cfg(feature = "dhat-heap")]
    let _profiler = dhat::Profiler::new_heap();

    let cli = cli::parse(version::display_version())?;
    if cli.command.show_alternative_help()? {
        return Ok(());
    }

    let signals = install_termination_signals().map_err(|e| anyhow!(e).context("failed to install signal handlers"))?;

    let color_enabled = Color::is_enabled(cli.color);
    let with_open_telemetry = cli.with_open_telemetry;
    let with_json_traces = cli.with_json_traces;
    let skip_logging = cli.command.skip_logging();
    let tui_settings = cli.command.tui_settings();
    // Capture observability hints before the command is consumed into a Runnable.
    let listen_address = cli.command.listen_address().map(str::to_owned);

    // Resolve the subcommand into a Runnable so we know which Tokio runtime to build.
    // Work is not started yet: the future is only created inside `run_on` after observability
    // is set up on that same runtime.
    let runnable = cli.command.into_runnable();
    let rt = runnable.build_runtime().map_err(|e| anyhow!(e).context("failed to build Tokio runtime"))?;
    #[cfg(feature = "counting-allocator")]
    let allocator_sampler = allocator_samples::Sampler::spawn_from_env(counting_allocator_snapshot)?;

    let with_tui = if !skip_logging
        && let Some(settings) = tui_settings.filter(|settings| tui::should_enable(settings.no_tui, with_json_traces))
    {
        let (_, config, startup) = settings.into_parts();
        set_signal_stderr_enabled(false);
        Some(tui::Session::spawn(config, startup, signals.shared_count())?)
    } else {
        set_signal_stderr_enabled(true);
        None
    };

    let OpenTelemetryHandle { meter, teardown } = if skip_logging {
        OpenTelemetryHandle::default()
    } else {
        // OpenTelemetry batch exporters require a current Tokio runtime.
        let _enter = rt.enter();
        let local = with_tui.as_ref().map(|tui| LocalTelemetry {
            metrics_observer: Some(tui.metrics_observer()),
            capture_layer: Some(tui.tracing_layer()),
        });
        let handle = setup_observability(
            with_open_telemetry,
            with_json_traces,
            local,
            color_enabled,
            &ListenAddressHint(listen_address.as_deref()),
        );
        // Record precise binary identity in operator logs as soon as tracing is live.
        version::log_build_version();
        handle
    };

    let result = runnable.run_on(&rt, &signals, meter);

    // Keep the runtime alive while OTEL providers flush (their batch tasks were spawned on it).
    if let Err(err) = run_teardown_with_timeout(teardown, Duration::from_secs(10)) {
        eprintln!("amaru: failed to teardown tracing: {err}");
    }

    if let Some(tui) = with_tui {
        if let Err(err) = tui.shutdown() {
            eprintln!("amaru: failed to shutdown terminal dashboard cleanly: {err}");
        }

        if let Err(ref err) = result {
            eprintln!("amaru: {err}");
        }
    }

    #[cfg(feature = "counting-allocator")]
    if let Some(sampler) = allocator_sampler
        && let Err(err) = sampler.shutdown()
    {
        eprintln!("amaru: failed to shutdown allocator sampler cleanly: {err}");
    }

    rt.shutdown_timeout(RUNTIME_SHUTDOWN_TIMEOUT);

    result
}

#[cfg(feature = "counting-allocator")]
fn counting_allocator_snapshot() -> AllocationSnapshot {
    GLOBAL.snapshot()
}

/// Thin adapter so we can pass a captured listen address after the clap command is consumed.
struct ListenAddressHint<'a>(Option<&'a str>);

impl ObservabilityHints for ListenAddressHint<'_> {
    fn listen_address(&self) -> Option<&str> {
        self.0
    }
}

fn run_teardown_with_timeout(
    teardown: Box<dyn FnOnce() -> Result<(), Box<dyn Error>> + Send>,
    timeout: Duration,
) -> Result<(), Box<dyn Error>> {
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let handle = std::thread::Builder::new()
        .name("amaru-otel-teardown".into())
        .spawn(move || {
            let result = teardown();
            let _ = done_tx.send(result.map_err(|e| e.to_string()));
        })
        .map_err(|e| anyhow!(e).context("failed to spawn observability teardown thread"))?;

    match done_rx.recv_timeout(timeout) {
        Ok(result) => {
            let _ = handle.join();
            result.map_err(|e| e.into())
        }
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            eprintln!("amaru: observability teardown timed out after {timeout:?}; continuing exit");
            Ok(())
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => match handle.join() {
            Ok(()) => Err(anyhow!("observability teardown ended without a result"))?,
            Err(_) => Err(anyhow!("observability teardown thread panicked"))?,
        },
    }
}
