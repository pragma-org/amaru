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
