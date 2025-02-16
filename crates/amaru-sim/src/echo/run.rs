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

use std::{
    io::{self},
    time::Duration,
};

use super::message::{
    Envelope,
    Message::{self, *},
};
use gasket::{framework::*, runtime::Tether};
use tokio_util::sync::CancellationToken;
use tracing::{info, trace};

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
    let mut echo = DoEcho::new();
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

type EchoesOut = gasket::messaging::OutputPort<Envelope>;
type EchoesIn = gasket::messaging::InputPort<Envelope>;

struct Action {}

struct ReadWorker {}

#[derive(Stage)]
#[stage(name = "read", unit = "Action", worker = "ReadWorker")]
struct ReadInput {
    downstream: EchoesOut,
}

impl ReadInput {
    fn new() -> Self {
        Self {
            downstream: EchoesOut::default(),
        }
    }
}

#[async_trait::async_trait(?Send)]
impl gasket::framework::Worker<ReadInput> for ReadWorker {
    async fn bootstrap(_stage: &ReadInput) -> Result<Self, WorkerError> {
        let worker = Self {};

        Ok(worker)
    }

    async fn schedule(
        &mut self,
        _stage: &mut ReadInput,
    ) -> Result<WorkSchedule<Action>, WorkerError> {
        Ok(WorkSchedule::Unit(Action {}))
    }

    async fn execute(&mut self, _unit: &Action, stage: &mut ReadInput) -> Result<(), WorkerError> {
        let mut input = String::new();
        io::stdin()
            .read_line(&mut input)
            .map_err(|_| WorkerError::Recv)?;

        info!("input '{}'", input);

        let msg: Envelope = serde_json::from_str(&input).map_err(|_| WorkerError::Recv)?;

        trace!("message '{:?}'", msg);

        stage
            .downstream
            .send(msg.into())
            .await
            .map_err(|_| WorkerError::Panic)?;
        Ok(())
    }
}

#[derive(Stage)]
#[stage(name = "do_echo", unit = "Action", worker = "EchoWorker")]
struct DoEcho {
    downstream: EchoesOut,
    upstream: EchoesIn,
    node_id: String,
    count: u64,
}

impl DoEcho {
    fn new() -> Self {
        Self {
            upstream: EchoesIn::default(),
            downstream: EchoesOut::default(),
            node_id: "".to_string(),
            count: 0,
        }
    }

    fn handle_echo(&mut self, msg: Envelope) -> Result<Envelope, WorkerError> {
        match msg.body {
            Init {
                msg_id,
                node_id,
                node_ids,
            } => {
                let body = self.init(msg_id, node_id, node_ids);
                Ok(Envelope {
                    src: self.node_id.clone(),
                    dest: msg.src,
                    body,
                })
            }
            Echo { msg_id, echo } => Ok(Envelope {
                src: self.node_id.clone(),
                dest: msg.src,
                body: self.echo(msg_id, echo),
            }),
            _ => Err(WorkerError::Panic),
        }
    }

    fn init(&mut self, msg_id: u64, node_id: String, _node_ids: Vec<String>) -> Message {
        self.node_id = node_id;
        InitOk {
            in_reply_to: msg_id,
        }
    }

    fn echo(&mut self, msg_id: u64, echo: String) -> Message {
        self.count += 1;
        if self.count % 5 == 0 {
            EchoOk {
                msg_id: self.count,
                in_reply_to: msg_id,
                echo: echo.to_uppercase(),
            }
        } else {
            EchoOk {
                msg_id: self.count,
                in_reply_to: msg_id,
                echo,
            }
        }
    }
}

struct EchoWorker {}

#[async_trait::async_trait(?Send)]
impl gasket::framework::Worker<DoEcho> for EchoWorker {
    async fn bootstrap(_stage: &DoEcho) -> Result<Self, WorkerError> {
        let worker = Self {};

        Ok(worker)
    }

    async fn schedule(&mut self, _stage: &mut DoEcho) -> Result<WorkSchedule<Action>, WorkerError> {
        Ok(WorkSchedule::Unit(Action {}))
    }

    async fn execute(&mut self, _unit: &Action, stage: &mut DoEcho) -> Result<(), WorkerError> {
        let from_echo = stage.upstream.recv().await.or_panic()?;

        let to_echo: Envelope = stage.handle_echo(from_echo.payload)?;

        stage
            .downstream
            .send(to_echo.into())
            .await
            .map_err(|_| WorkerError::Panic)
    }
}

#[derive(Stage)]
#[stage(name = "write_output", unit = "Action", worker = "OutputWorker")]
struct SendOutput {
    upstream: EchoesIn,
}

impl SendOutput {
    fn new() -> Self {
        Self {
            upstream: EchoesIn::default(),
        }
    }
}

struct OutputWorker {}

#[async_trait::async_trait(?Send)]
impl gasket::framework::Worker<SendOutput> for OutputWorker {
    async fn bootstrap(_stage: &SendOutput) -> Result<Self, WorkerError> {
        let worker = Self {};

        Ok(worker)
    }

    async fn schedule(
        &mut self,
        _stage: &mut SendOutput,
    ) -> Result<WorkSchedule<Action>, WorkerError> {
        Ok(WorkSchedule::Unit(Action {}))
    }

    async fn execute(&mut self, _unit: &Action, stage: &mut SendOutput) -> Result<(), WorkerError> {
        let from_echo = stage.upstream.recv().await.or_panic()?;
        let out = serde_json::to_string(&from_echo.payload).map_err(|_| WorkerError::Panic)?;
        trace!("Sending '{}'", out);
        println!("{}", out);
        Ok(())
    }
}
