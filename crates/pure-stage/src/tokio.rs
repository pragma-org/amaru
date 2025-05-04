use crate::{BoxFuture, Message, Name, StageBuildRef, StageGraph, StageRef, State};
use std::{future::Future, marker::PhantomData, sync::Arc};
use tokio::{
    spawn,
    sync::mpsc::{channel, Receiver},
    task::JoinHandle,
};

#[derive(Debug, thiserror::Error)]
#[error("message send failed to stage `{target}`")]
pub struct SendError {
    target: Name,
}

/// A [`StageGraph`] implementation that dispatches each stage as a task on the Tokio global pool.
///
/// *This is currently only a minimal sketch that will likely not fit the intended design.
/// It is more likely that the effect handling will be done like in the [`SimulationBuilder`](crate::simulation::SimulationBuilder)
/// implementation.*
pub struct TokioBuilder {
    tasks: Vec<BoxFuture<'static, anyhow::Result<()>>>,
}

impl StageGraph for TokioBuilder {
    type Running = TokioRunning;
    type RefAux<Msg, State> = (
        Receiver<Msg>,
        Box<dyn FnMut(State, Msg) -> BoxFuture<'static, anyhow::Result<State>> + 'static + Send>,
    );

    fn stage<Msg: Message, St: State, F, Fut>(
        &mut self,
        name: impl AsRef<str>,
        mut f: F,
        state: St,
    ) -> StageBuildRef<Msg, St, Self::RefAux<Msg, St>>
    where
        F: FnMut(St, Msg) -> Fut + 'static + Send,
        Fut: Future<Output = anyhow::Result<St>> + 'static + Send,
    {
        let name = Name::from(name.as_ref());
        let (tx, rx) = channel(10);
        StageBuildRef {
            name: name.clone(),
            state,
            network: (rx, Box::new(move |state, msg| Box::pin(f(state, msg)))),
            send: Arc::new(move |msg| {
                let tx = tx.clone();
                let target = name.clone();
                Box::pin(async move {
                    tx.send(msg)
                        .await
                        .map_err(|_| anyhow::Error::from(SendError { target }))
                })
            }),
        }
    }

    fn wire_up<Msg: Message, St: State>(
        &mut self,
        stage: StageBuildRef<Msg, St, Self::RefAux<Msg, St>>,
        f: impl FnOnce(&mut St),
    ) -> StageRef<Msg, St> {
        let StageBuildRef {
            name,
            mut state,
            network: (mut rx, mut ff),
            send,
        } = stage;
        f(&mut state);
        let stage_name = name.clone();
        self.tasks.push(Box::pin(async move {
            while let Some(msg) = rx.recv().await {
                state = ff(state, msg).await.inspect_err(|err| {
                    tracing::error!("stage `{}` error: {:?}", stage_name, err);
                })?;
            }
            Ok(())
        }));
        StageRef {
            name,
            send,
            _ph: PhantomData,
        }
    }

    fn run(self) -> Self::Running {
        let handles = self.tasks.into_iter().map(spawn).collect();
        TokioRunning { handles }
    }
}

/// Handle to the running stages.
#[must_use = "this handle needs to be either joined or aborted"]
pub struct TokioRunning {
    handles: Vec<JoinHandle<anyhow::Result<()>>>,
}

impl TokioRunning {
    /// Abort all stage tasks of this network.
    pub fn abort(self) {
        for handle in self.handles {
            handle.abort();
        }
    }

    pub async fn join(self) -> Vec<anyhow::Result<()>> {
        let mut res = Vec::new();
        for handle in self.handles.into_iter() {
            res.push(handle.await.unwrap_or_else(|err| Err(err.into())));
        }
        res
    }
}
