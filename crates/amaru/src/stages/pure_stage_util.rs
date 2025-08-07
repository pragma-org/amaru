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

use futures_util::StreamExt;
use gasket::{
    framework::{Stage, WorkSchedule, WorkerError},
    messaging,
};
use pure_stage::{tokio::TokioRunning, Receiver, SendData, Sender};
use std::future::pending;
use tokio::runtime::Runtime;

#[derive(Stage)]
#[stage(name = "pure_stage", unit = "()", worker = "Worker")]
pub struct PureStageSim {
    _tokio_running: TokioRunning,
    _runtime: Runtime,
}

impl PureStageSim {
    pub fn new(tokio_running: TokioRunning, runtime: Runtime) -> Self {
        Self {
            _tokio_running: tokio_running,
            _runtime: runtime,
        }
    }
}

// Worker acts as a placeholder - the actual work is handled by the pure-stage runtime
// so this just needs to satisfy gasket's Worker trait requirements.
pub struct Worker;

#[async_trait::async_trait(?Send)]
impl gasket::framework::Worker<PureStageSim> for Worker {
    async fn bootstrap(_stage: &PureStageSim) -> Result<Self, WorkerError> {
        Ok(Worker)
    }

    async fn schedule(
        &mut self,
        _stage: &mut PureStageSim,
    ) -> Result<WorkSchedule<()>, WorkerError> {
        // never any work to do here, PureStageSim only needs to keep the
        // tokio runtime and the pure_stage tasks running
        pending().await
    }

    async fn execute(&mut self, _unit: &(), _stage: &mut PureStageSim) -> Result<(), WorkerError> {
        Ok(())
    }
}

pub struct SendAdapter<Msg>(pub Sender<Msg>);

#[async_trait::async_trait]
impl<Msg: SendData> messaging::SendAdapter<Msg> for SendAdapter<Msg> {
    async fn send(&mut self, msg: messaging::Message<Msg>) -> Result<(), gasket::error::Error> {
        self.0
            .send(msg.payload)
            .await
            .map_err(|_| gasket::error::Error::NotConnected)
    }
}

pub struct RecvAdapter<Msg>(pub Receiver<Msg>);

#[async_trait::async_trait]
impl<Msg: Send + Sync + Clone> messaging::RecvAdapter<Msg> for RecvAdapter<Msg> {
    async fn recv(&mut self) -> Result<messaging::Message<Msg>, gasket::error::Error> {
        self.0
            .next()
            .await
            .ok_or(gasket::error::Error::NotConnected)
            .map(|msg| msg.into())
    }
}
