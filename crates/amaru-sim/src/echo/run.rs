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

use std::time::Duration;

use crate::echo::{
    io::{DoEcho, ReadInput, SendOutput},
    service::EchoService,
};

use gasket::runtime::Tether;
use tokio_util::sync::CancellationToken;
use tracing::trace;

pub async fn run() {
    let echo_pipeline = bootstrap();

    let exit = amaru::exit::hook_exit_token();

    trace!("starting pipeline");

    run_pipeline(gasket::daemon::Daemon::new(echo_pipeline), exit.clone()).await;
}

/// Bootstrap gasket pipeline for the echo service.
///
/// The echo pipeline consists of three stages:
/// 1. ReadInput: reads a JSON string from stdin, decode it, and pass it to
///    the next stage
/// 2. DoEcho: receives a message, echoes it to the next stage.
/// 3. SendOutput: receives the echoed message and prints it to stdout as a
///    JSON.
pub fn bootstrap() -> Vec<Tether> {
    let mut read_input = ReadInput::new();
    let service = Box::new(EchoService::new());
    let mut echo = DoEcho::new(service);
    let mut send_output = SendOutput::new();

    let (to_echo, from_in) = gasket::messaging::tokio::mpsc_channel(50);
    let (to_out, from_echo) = gasket::messaging::tokio::mpsc_channel(50);

    read_input.downstream.connect(to_echo);
    echo.upstream.connect(from_in);
    echo.downstream.connect(to_out);
    send_output.upstream.connect(from_echo);

    let policy = define_gasket_policy();

    let read_input = gasket::runtime::spawn_stage(read_input, policy.clone());
    let echo = gasket::runtime::spawn_stage(echo, policy.clone());
    let send_output = gasket::runtime::spawn_stage(send_output, policy.clone());

    vec![read_input, echo, send_output]
}

pub async fn run_pipeline(pipeline: gasket::daemon::Daemon, exit: CancellationToken) {
    loop {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(1000)) => {
                if pipeline.should_stop() {
                    break;
                }
            }
            _ = exit.cancelled() => {
                trace!("exit requested");
                break;
            }
        }
    }

    trace!("shutting down pipeline");

    pipeline.teardown();
}

fn define_gasket_policy() -> gasket::runtime::Policy {
    let retries = gasket::retries::Policy {
        max_retries: 20,
        backoff_unit: std::time::Duration::from_secs(1),
        backoff_factor: 2,
        max_backoff: std::time::Duration::from_secs(60),
        dismissible: false,
    };

    gasket::runtime::Policy {
        //be generous with tick timeout to avoid timeout during block awaits
        tick_timeout: std::time::Duration::from_secs(600).into(),
        bootstrap_retry: retries.clone(),
        work_retry: retries.clone(),
        teardown_retry: retries.clone(),
    }
}
