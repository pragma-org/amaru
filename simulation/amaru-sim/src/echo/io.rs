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

use super::{message::Envelope, service::EchoService, EchoMessage};
/// This module contains the streams, stages and workers to encapsulate the echo service.
/// This could probably be made more generic as it's pretty much boilerplate adapter
/// between maelstrom protocol, gasket framework, and the underlying service receiving
/// and sending messages.
use futures_util::sink::SinkExt;
use gasket::framework::*;
use tokio::io::{stdin, stdout};
use tokio_stream::StreamExt;
use tokio_util::codec::{FramedRead, FramedWrite, LinesCodec};
use tracing::trace;

/// The input ports for the service.
pub type EnvelopeOut = gasket::messaging::OutputPort<Envelope<EchoMessage>>;

/// The output ports for the service.
pub type EnvelopeIn = gasket::messaging::InputPort<Envelope<EchoMessage>>;

/// A unit of work for the echo service.
/// There's not much to do here, just a placeholder.
pub struct Action {}

pub struct ReadWorker {}

/// The stage that reads a single line of JSON-formatted `Envelope` from stdin
/// and propagates it downstream to the service.
#[derive(Stage)]
#[stage(name = "read", unit = "Action", worker = "ReadWorker")]
pub struct ReadInput {
    pub downstream: EnvelopeOut,
}

impl ReadInput {
    pub fn new() -> Self {
        Self {
            downstream: EnvelopeOut::default(),
        }
    }

    pub async fn read_input(&self) -> Result<Envelope<EchoMessage>, WorkerError> {
        let mut reader = FramedRead::new(stdin(), LinesCodec::new());
        let input = reader.next().await;

        match input {
            Some(input) => {
                let line = input.map_err(|_| WorkerError::Retry)?;
                trace!("input '{}'", line);
                serde_json::from_str(&line).map_err(|_| WorkerError::Retry)
            }
            None => Err(WorkerError::Panic), // EOF
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
        let msg = stage.read_input().await.or_panic()?;

        stage
            .downstream
            .send(msg.into())
            .await
            .map_err(|_| WorkerError::Panic)?;
        Ok(())
    }
}

/// The stage that sends `Envelope`s received from the service to `stdout`.
#[derive(Stage)]
#[stage(name = "write_output", unit = "Action", worker = "OutputWorker")]
pub struct SendOutput {
    pub upstream: EnvelopeIn,
}

impl SendOutput {
    pub fn new() -> Self {
        Self {
            upstream: EnvelopeIn::default(),
        }
    }

    pub async fn write_output(&self, msg: Envelope<EchoMessage>) -> Result<(), WorkerError> {
        let mut writer = FramedWrite::new(stdout(), LinesCodec::new());
        writer
            .send(serde_json::to_string(&msg).map_err(|_| WorkerError::Panic)?)
            .await
            .or_panic()
    }
}

pub struct OutputWorker {}

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
        stage.write_output(from_echo.payload).await
    }
}

/// Stage wrapper around the `EchoService`.
#[derive(Stage)]
#[stage(name = "do_echo", unit = "Action", worker = "EchoWorker")]
pub struct DoEcho {
    pub downstream: EnvelopeOut,
    pub upstream: EnvelopeIn,
    echo_service: Box<EchoService>,
}
impl DoEcho {
    pub(crate) fn new(echo_service: Box<EchoService>) -> Self {
        Self {
            downstream: EnvelopeOut::default(),
            upstream: EnvelopeIn::default(),
            echo_service,
        }
    }
}

pub struct EchoWorker {}

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

        let to_echo: Envelope<EchoMessage> = stage
            .echo_service
            .handle_echo(from_echo.payload)
            .map_err(|_| WorkerError::Recv)?;

        stage
            .downstream
            .send(to_echo.into())
            .await
            .map_err(|_| WorkerError::Panic)
    }
}
