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

//! This module contains the Tokio-based [`StageGraph`] implementation, to be used in production.
//!
//! It is good practice to perform the stage contruction and wiring in a function that takes an
//! `&mut impl StageGraph` so that it can be reused between the Tokio and simulation implementations.

use crate::stage_ref::Stage;
use crate::{
    BoxFuture, Effects, Instant, Name, SendData, Sender, StageBuildRef, StageGraph, StageRef,
    effect::{StageEffect, StageResponse},
    resources::Resources,
    simulation::EffectBox,
    stagegraph::StageGraphRunning,
    time::Clock,
};
use either::Either::{Left, Right};
use parking_lot::Mutex;
use std::{
    collections::BTreeMap,
    future::Future,
    marker::PhantomData,
    sync::Arc,
    task::{Context, Poll, Waker},
};
use tokio::{
    runtime::Handle,
    sync::{
        mpsc::{self, Receiver},
        watch,
    },
    task::JoinHandle,
};

#[derive(Debug, thiserror::Error)]
#[error("message send failed to stage `{target}`")]
pub struct SendError {
    target: Name,
}

struct TokioInner {
    senders: BTreeMap<Name, mpsc::Sender<Box<dyn SendData>>>,
    clock: Arc<dyn Clock + Send + Sync>,
    resources: Resources,
    mailbox_size: usize,
    termination: watch::Sender<bool>,
}

impl TokioInner {
    fn new(termination: watch::Sender<bool>) -> Self {
        Self {
            senders: Default::default(),
            clock: Arc::new(TokioClock),
            resources: Resources::default(),
            mailbox_size: 10,
            termination,
        }
    }
}

struct TokioClock;
impl Clock for TokioClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
    fn advance_to(&self, _instant: Instant) {}
}

/// A [`StageGraph`] implementation that dispatches each stage as a task on the Tokio global pool.
///
/// *This is currently only a minimal sketch that will likely not fit the intended design.
/// It is more likely that the effect handling will be done like in the [`SimulationBuilder`](crate::simulation::SimulationBuilder)
/// implementation.*
pub struct TokioBuilder {
    tasks: Vec<Box<dyn FnOnce(Arc<TokioInner>) -> BoxFuture<'static, ()>>>,
    inner: TokioInner,
    termination: watch::Receiver<bool>,
}

impl Default for TokioBuilder {
    fn default() -> Self {
        let (termination, termination_rx) = watch::channel(false);
        Self {
            tasks: Default::default(),
            inner: TokioInner::new(termination),
            termination: termination_rx,
        }
    }
}

impl StageGraph for TokioBuilder {
    type Running = TokioRunning;

    type RefAux<Msg, State> = (
        Receiver<Box<dyn SendData>>,
        Box<dyn FnMut(State, Msg, Effects<Msg>) -> BoxFuture<'static, State> + 'static + Send>,
    );

    fn stage<Msg: SendData, St: SendData, F, Fut>(
        &mut self,
        name: impl AsRef<str>,
        mut f: F,
    ) -> StageBuildRef<Msg, St, Self::RefAux<Msg, St>>
    where
        F: FnMut(St, Msg, Effects<Msg>) -> Fut + 'static + Send,
        Fut: Future<Output = St> + 'static + Send,
    {
        // THIS MUST MATCH THE SIMULATION BUILDER
        let name = Name::from(&*format!("{}-{}", name.as_ref(), self.inner.senders.len()));
        let (tx, rx) = mpsc::channel(self.inner.mailbox_size);
        self.inner.senders.insert(name.clone(), tx);
        StageBuildRef {
            name,
            network: (
                rx,
                Box::new(move |state, msg, eff| Box::pin(f(state, msg, eff))),
            ),
            _ph: PhantomData,
        }
    }

    #[allow(clippy::expect_used)]
    fn wire_up<Msg, St>(
        &mut self,
        stage: StageBuildRef<Msg, St, Self::RefAux<Msg, St>>,
        state: St,
    ) -> StageRef<Msg>
    where
        Msg: SendData + serde::de::DeserializeOwned,
        St: SendData,
    {
        self.wire(stage, state).as_ref()
    }

    #[allow(clippy::expect_used)]
    fn wire<Msg, St>(
        &mut self,
        stage: StageBuildRef<Msg, St, Self::RefAux<Msg, St>>,
        mut state: St,
    ) -> Stage<Msg, St>
    where
        Msg: SendData + serde::de::DeserializeOwned,
        St: SendData,
    {
        let StageBuildRef {
            name,
            network: (mut rx, mut ff),
            _ph,
        } = stage;
        let stage_name = name.clone();
        self.tasks.push(Box::new(move |inner| {
            Box::pin(async move {
                let me = StageRef {
                    name: stage_name.clone(),
                    _ph: PhantomData,
                };
                let effect = Arc::new(Mutex::new(None));
                let sender = mk_sender(&stage_name, &inner);
                let effects = Effects::new(me, effect.clone(), inner.clock.clone(), sender);
                while let Some(msg) = rx.recv().await {
                    let result = interpreter(
                        &inner,
                        &effect,
                        &stage_name,
                        ff(
                            state,
                            *msg.cast::<Msg>().expect("internal message type error"),
                            effects.clone(),
                        ),
                    )
                    .await;
                    match result {
                        Some(st) => state = st,
                        None => {
                            tracing::info!(%stage_name, "terminated");
                            inner.termination.send_replace(true);
                            break;
                        }
                    }
                }
            })
        }));
        Stage::new(name)
    }

    #[allow(clippy::expect_used)]
    fn input<Msg: SendData>(&mut self, stage: &StageRef<Msg>) -> Sender<Msg> {
        mk_sender(&stage.name, &self.inner)
    }

    fn run(self, rt: Handle) -> Self::Running {
        let Self {
            tasks,
            inner,
            termination,
        } = self;
        let inner = Arc::new(inner);
        let handles = Arc::new(Mutex::new(
            tasks
                .into_iter()
                .map(|t| rt.spawn(t(inner.clone())))
                .collect::<Vec<_>>(),
        ));

        // abort all tasks as soon as the termination signal is received
        let mut termination2 = termination.clone();
        let handles2 = handles.clone();
        rt.spawn(async move {
            termination2.wait_for(|x| *x).await.ok();
            for handle in handles2.lock().iter() {
                handle.abort();
            }
        });

        TokioRunning {
            handles,
            termination,
        }
    }

    fn resources(&self) -> &Resources {
        &self.inner.resources
    }
}

#[allow(clippy::expect_used)]
fn mk_sender<Msg: SendData>(stage_name: &Name, inner: &TokioInner) -> Sender<Msg> {
    let tx = inner
        .senders
        .get(stage_name)
        .expect("stage ref contained unknown name")
        .clone();
    Sender::new(Arc::new(move |msg: Msg| {
        let tx = tx.clone();
        Box::pin(async move {
            tx.send(Box::new(msg))
                .await
                .map_err(|msg| *msg.0.cast::<Msg>().expect("message was just boxed"))
        })
    }))
}

async fn interpreter<St>(
    inner: &TokioInner,
    effect: &EffectBox,
    name: &Name,
    mut stage: BoxFuture<'static, St>,
) -> Option<St> {
    loop {
        let poll = stage.as_mut().poll(&mut Context::from_waker(Waker::noop()));
        if let Poll::Ready(state) = poll {
            return Some(state);
        }
        #[allow(clippy::panic)]
        let Some(Left(eff)) = effect.lock().take() else {
            panic!("stage `{name}` used .await on something that was not a stage effect");
        };
        let resp = match eff {
            StageEffect::Receive => {
                #[allow(clippy::panic)]
                {
                    panic!("effect Receive cannot be explicitly awaited (stage `{name}`)")
                }
            }
            StageEffect::Send(target, msg, call) => {
                #[allow(clippy::expect_used)]
                let tx = inner
                    .senders
                    .get(&target)
                    .expect("stage ref contained unknown name");
                let Ok(_) = tx.send(msg).await else {
                    tracing::warn!("message send failed from stage `{name}` to stage `{target}`");
                    return None;
                };
                if let Some((d, rx, _id)) = call {
                    tokio::time::timeout(d, rx)
                        .await
                        .ok()
                        .and_then(|r| r.ok())
                        .map(StageResponse::CallResponse)
                        .unwrap_or(StageResponse::CallTimeout)
                } else {
                    StageResponse::Unit
                }
            }
            StageEffect::Clock => StageResponse::ClockResponse(now()),
            StageEffect::Wait(duration) => {
                tokio::time::sleep(duration).await;
                StageResponse::WaitResponse(now())
            }
            StageEffect::Call(..) => {
                #[allow(clippy::panic)]
                {
                    panic!("StageEffect::Call cannot be explicitly awaited (stage `{name}`")
                }
            }
            StageEffect::Respond(target, _call_id, deadline, sender, msg) => {
                if let Err(msg) = sender.send(msg) {
                    tracing::warn!(
                        "response to {} was dropped: {:?} (deadline: {})",
                        target,
                        msg,
                        deadline.pretty(now())
                    );
                }
                StageResponse::Unit
            }
            StageEffect::External(effect) => {
                tracing::debug!("stage `{name}` external effect: {:?}", effect);
                StageResponse::ExternalResponse(effect.run(inner.resources.clone()).await)
            }
            StageEffect::Terminate => {
                tracing::warn!("stage `{name}` terminated");
                return None;
            }
        };
        *effect.lock() = Some(Right(resp));
    }
}

fn now() -> Instant {
    Instant::from_tokio(tokio::time::Instant::now())
}

/// Handle to the running stages.
#[must_use = "this handle needs to be either joined or aborted"]
pub struct TokioRunning {
    handles: Arc<Mutex<Vec<JoinHandle<()>>>>,
    termination: watch::Receiver<bool>,
}

impl TokioRunning {
    /// Abort all stage tasks of this network.
    pub fn abort(self) {
        for handle in self.handles.lock().iter() {
            handle.abort();
        }
    }

    pub async fn join(self) {
        let handles = std::mem::take(&mut *self.handles.lock());
        for handle in handles {
            handle.await.unwrap_or_else(|err| {
                tracing::error!("stage task failed: {:?}", err);
            });
        }
    }
}

impl StageGraphRunning for TokioRunning {
    fn is_terminated(&self) -> bool {
        *self.termination.borrow()
    }

    fn termination(&self) -> BoxFuture<'static, ()> {
        let mut rx = self.termination.clone();
        Box::pin(async move {
            rx.wait_for(|x| *x).await.ok();
        })
    }
}
