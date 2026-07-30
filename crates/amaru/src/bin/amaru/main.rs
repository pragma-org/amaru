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
    observability::{Color, setup_observability},
    panic::panic_handler,
    version,
};

mod cli;
mod cmd;
mod pid;

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

    let (metrics, teardown) = if cli.command.skip_logging() {
        (None, Box::new(|| Ok(())) as Box<dyn FnOnce() -> Result<(), Box<dyn Error>> + Send>)
    } else {
        let (m, t) = setup_observability(cli.with_open_telemetry, cli.with_json_traces, color_enabled, &cli.command);
        (Some(m), t)
    };

    let result = cli.command.into_runnable(metrics.unwrap_or(None)).run(&signals);

    if let Err(report) = run_teardown_with_timeout(teardown, Duration::from_secs(10)) {
        eprintln!("Failed to teardown tracing: {report}");
    }

    result
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
