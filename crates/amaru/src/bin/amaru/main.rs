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
    lifecycle::RUNTIME_SHUTDOWN_TIMEOUT,
    observability::{Color, ObservabilityHints, setup_observability},
    panic::panic_handler,
    version,
};
use amaru_tui as tui;
use mimalloc::MiMalloc;

mod cli;
mod cmd;
mod pid;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

fn main() -> ExitCode {
    panic_handler();

    match try_main() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("amaru: {err}");
            ExitCode::FAILURE
        }
    }
}

fn try_main() -> Result<(), Box<dyn Error>> {
    let cli = cli::parse(version::display_version())?;
    if cli.command.show_alternative_help()? {
        return Ok(());
    }

    let signals = install_termination_signals().map_err(|e| format!("failed to install signal handlers: {e}"))?;

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
    let rt = runnable.build_runtime().map_err(|e| format!("failed to build Tokio runtime: {e}"))?;

    let tui = if skip_logging {
        None
    } else {
        tui_settings
            .filter(|settings| tui::should_enable(settings.no_tui, with_json_traces))
            .map(|settings| {
                let (_, config, startup) = settings.into_parts();
                tui::Session::spawn(config, startup)
            })
            .transpose()?
    };
    let _metrics_subscription = tui.as_ref().map(tui::Session::subscribe_to_metrics);

    let (metrics, teardown) = if skip_logging {
        (None, Box::new(|| Ok(())) as Box<dyn FnOnce() -> Result<(), Box<dyn Error>> + Send>)
    } else {
        // OpenTelemetry batch exporters require a current Tokio runtime.
        let _enter = rt.enter();
        setup_observability(
            with_open_telemetry,
            with_json_traces,
            color_enabled,
            &ListenAddressHint(listen_address.as_deref()),
            tui.as_ref().map(tui::Session::layer),
        )
    };

    let result = runnable.run_on(&rt, &signals, metrics);

    // Keep the runtime alive while OTEL providers flush (their batch tasks were spawned on it).
    if let Err(report) = run_teardown_with_timeout(teardown, Duration::from_secs(10)) {
        eprintln!("Failed to teardown tracing: {report}");
    }

    if let Some(tui) = tui
        && let Err(err) = tui.shutdown()
    {
        eprintln!("amaru: failed to shutdown terminal dashboard cleanly: {err}");
    }

    rt.shutdown_timeout(RUNTIME_SHUTDOWN_TIMEOUT);

    result
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
        .map_err(|e| format!("failed to spawn observability teardown thread: {e}"))?;

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
            Ok(()) => Err("observability teardown ended without a result".into()),
            Err(_) => Err("observability teardown thread panicked".into()),
        },
    }
}
