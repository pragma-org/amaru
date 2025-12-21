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

use crate::adapter::{Adapter, StageOrAdapter, find_recipient};
use crate::simulation::Transition;
use crate::stage_name;
use crate::stage_ref::StageStateRef;
use crate::trace_buffer::TraceBuffer;
use crate::{
    BoxFuture, Effects, Instant, Name, SendData, Sender, StageBuildRef, StageGraph, StageRef,
    effect::{StageEffect, StageResponse},
    effect_box::EffectBox,
    resources::Resources,
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
    senders: Mutex<BTreeMap<Name, StageOrAdapter<mpsc::Sender<Box<dyn SendData>>>>>,
    handles: Mutex<Vec<JoinHandle<()>>>,
    clock: Arc<dyn Clock + Send + Sync>,
    resources: Resources,
    mailbox_size: usize,
    termination: watch::Sender<bool>,
    stage_counter: Mutex<usize>,
    trace_buffer: Arc<Mutex<TraceBuffer>>,
}

impl TokioInner {
    fn new(termination: watch::Sender<bool>) -> Self {
        Self {
            senders: Default::default(),
            handles: Default::default(),
            clock: Arc::new(TokioClock),
            resources: Resources::default(),
            mailbox_size: 10,
            termination,
            stage_counter: Mutex::new(0usize),
            trace_buffer: TraceBuffer::new_shared(0, 0),
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
/// It is more likely that the effect handling will be done like in the [`SimulationBuilder`](crate::effect_box::SimulationBuilder)
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

impl TokioBuilder {
    pub fn run(self, rt: Handle) -> TokioRunning {
        let Self {
            tasks,
            inner,
            termination,
        } = self;
        let inner = Arc::new(inner);
        let handles = tasks
            .into_iter()
            .map(|t| rt.spawn(t(inner.clone())))
            .collect::<Vec<_>>();
        inner.handles.lock().extend(handles);

        // abort all tasks as soon as the termination signal is received
        let mut termination2 = termination.clone();
        let inner2 = inner.clone();
        rt.spawn(async move {
            termination2.wait_for(|x| *x).await.ok();
            for handle in inner2.handles.lock().iter() {
                handle.abort();
            }
        });

        TokioRunning { inner, termination }
    }
}

impl StageGraph for TokioBuilder {
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
        let name = stage_name(&mut self.inner.stage_counter.lock(), name.as_ref());
        let (tx, rx) = mpsc::channel(self.inner.mailbox_size);
        self.inner
            .senders
            .lock()
            .insert(name.clone(), StageOrAdapter::Stage(tx));
        StageBuildRef {
            name,
            network: (
                rx,
                Box::new(move |state, msg, eff| Box::pin(f(state, msg, eff))),
            ),
            _ph: PhantomData,
        }
    }

    fn wire_up<Msg: SendData, St: SendData>(
        &mut self,
        stage: StageBuildRef<Msg, St, Self::RefAux<Msg, St>>,
        state: St,
    ) -> StageStateRef<Msg, St> {
        let StageBuildRef {
            name,
            network: (rx, ff),
            _ph,
        } = stage;
        let stage_name = name.clone();
        self.tasks.push(Box::new(move |inner| {
            Box::pin(run_stage(state, rx, ff, stage_name, inner))
        }));
        StageStateRef::new(name)
    }

    fn contramap<Original: SendData, Mapped: SendData>(
        &mut self,
        stage_ref: impl AsRef<StageRef<Original>>,
        new_name: impl AsRef<str>,
        transform: impl Fn(Mapped) -> Original + 'static + Send,
    ) -> StageRef<Mapped> {
        let target = stage_ref.as_ref();
        let new_name = stage_name(&mut self.inner.stage_counter.lock(), new_name.as_ref());
        let adapter = Adapter::new(new_name.clone(), target.name().clone(), transform);
        self.inner
            .senders
            .lock()
            .insert(new_name.clone(), StageOrAdapter::Adapter(adapter));
        StageRef::new(new_name)
    }

    fn preload<Msg: SendData>(
        &mut self,
        stage: impl AsRef<StageRef<Msg>>,
        messages: impl IntoIterator<Item = Msg>,
    ) -> Result<(), Box<dyn SendData>> {
        let stage = stage.as_ref();
        let mut senders = self.inner.senders.lock();
        for msg in messages {
            if let Some((tx, msg)) =
                find_recipient(&mut senders, stage.name().clone(), Some(Box::new(msg)))
                && let Err(err) = tx.try_send(msg)
            {
                tracing::warn!("message preload failed to stage `{}`", stage.name());
                return Err(err.into_inner());
            }
        }
        Ok(())
    }

    fn input<Msg: SendData>(&mut self, stage: impl AsRef<StageRef<Msg>>) -> Sender<Msg> {
        mk_sender(stage.as_ref().name(), &self.inner)
    }

    fn resources(&self) -> &Resources {
        &self.inner.resources
    }
}

async fn run_stage<Msg: SendData, St: SendData>(
    mut state: St,
    mut rx: Receiver<Box<dyn SendData + 'static>>,
    mut ff: Box<
        dyn FnMut(
                St,
                Msg,
                Effects<Msg>,
            ) -> std::pin::Pin<Box<dyn Future<Output = St> + Send + 'static>>
            + Send,
    >,
    stage_name: Name,
    inner: Arc<TokioInner>,
) {
    tracing::debug!("running stage `{stage_name}`");
    let me = StageRef::new(stage_name.clone());
    let effect = Arc::new(Mutex::new(None));
    let effects = Effects::new(
        me,
        effect.clone(),
        inner.clock.clone(),
        inner.resources.clone(),
        inner.trace_buffer.clone(),
    );
    while let Some(msg) = rx.recv().await {
        let result = interpreter(
            &inner,
            &effect,
            &stage_name,
            ff(
                state,
                #[expect(clippy::expect_used)]
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
}

async fn run_stage_boxed(
    mut state: Box<dyn SendData>,
    mut rx: Receiver<Box<dyn SendData + 'static>>,
    mut transition: Transition,
    effect: EffectBox,
    stage_name: Name,
    inner: Arc<TokioInner>,
) {
    tracing::debug!("running stage `{stage_name}`");
    while let Some(msg) = rx.recv().await {
        let result = interpreter(&inner, &effect, &stage_name, (transition)(state, msg)).await;
        match result {
            Some(st) => state = st,
            None => {
                tracing::info!(%stage_name, "terminated");
                inner.termination.send_replace(true);
                break;
            }
        }
    }
}

#[expect(clippy::expect_used, clippy::panic)]
fn mk_sender<Msg: SendData>(stage_name: &Name, inner: &TokioInner) -> Sender<Msg> {
    let senders = inner.senders.lock();
    let StageOrAdapter::Stage(tx) = senders
        .get(stage_name)
        .expect("stage ref contained unknown name")
    else {
        panic!("cannot obtain input for adapter");
    };
    let tx = tx.clone();
    Sender::new(Arc::new(move |msg: Msg| {
        let tx = tx.clone();
        Box::pin(async move {
            tx.send(Box::new(msg))
                .await
                .map_err(|msg| *msg.0.cast::<Msg>().expect("message was just boxed"))
        })
    }))
}

// clippy is lying, changing to async fn does not work.
#[expect(clippy::manual_async_fn)]
fn interpreter<St>(
    inner: &Arc<TokioInner>,
    effect: &EffectBox,
    name: &Name,
    mut stage: BoxFuture<'static, St>,
) -> impl Future<Output = Option<St>> + Send {
    // trying to write this as an async fn fails with inscrutable compile errors, it seems
    // that rustc has some issue with this particular pattern
    async move {
        loop {
            let poll = stage.as_mut().poll(&mut Context::from_waker(Waker::noop()));
            if let Poll::Ready(state) = poll {
                return Some(state);
            }
            drop(poll);

            #[expect(clippy::panic)]
            let Some(Left(eff)) = effect.lock().take() else {
                panic!("stage `{name}` used .await on something that was not a stage effect");
            };
            let resp = match eff {
                StageEffect::Receive => {
                    #[expect(clippy::panic)]
                    {
                        panic!("effect Receive cannot be explicitly awaited (stage `{name}`)")
                    }
                }
                StageEffect::Send(target, msg, call) => {
                    if target.as_str().is_empty() {
                        if let Some((duration, _, _)) = call {
                            tracing::warn!(stage = %name, "call to blackhole stage dropped");
                            tokio::time::sleep(duration).await;
                            StageResponse::CallTimeout
                        } else {
                            tracing::warn!(stage = %name, "message send to blackhole stage dropped");
                            StageResponse::Unit
                        }
                    } else {
                        let (tx, msg) = {
                            let mut senders = inner.senders.lock();
                            #[expect(clippy::expect_used)]
                            let (tx, msg) = find_recipient(&mut senders, target.clone(), Some(msg))
                                .expect("stage ref contained unknown name");
                            (tx.clone(), msg)
                        };
                        let Ok(_) = tx.send(msg).await else {
                            tracing::warn!(
                                "message send failed from stage `{name}` to stage `{target}`"
                            );
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
                }
                StageEffect::Clock => StageResponse::ClockResponse(now()),
                StageEffect::Wait(duration) => {
                    tokio::time::sleep(duration).await;
                    StageResponse::WaitResponse(now())
                }
                StageEffect::Call(..) => {
                    #[expect(clippy::panic)]
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
                StageEffect::AddStage(name) => {
                    tracing::debug!("stage `{name}` added");
                    let name = stage_name(&mut inner.stage_counter.lock(), name.as_str());
                    StageResponse::AddStageResponse(name)
                }
                StageEffect::WireStage(name, transition, initial_state) => {
                    tracing::debug!("stage `{name}` wired");
                    let (tx, rx) = mpsc::channel(inner.mailbox_size);
                    inner
                        .senders
                        .lock()
                        .insert(name.clone(), StageOrAdapter::Stage(tx));
                    let effect = Arc::new(Mutex::new(None));
                    inner.handles.lock().push(tokio::spawn(run_stage_boxed(
                        initial_state,
                        rx,
                        (transition.into_inner())(effect.clone()),
                        effect,
                        name,
                        inner.clone(),
                    )));
                    StageResponse::Unit
                }
                StageEffect::Contramap {
                    original,
                    new_name,
                    transform,
                } => {
                    tracing::debug!("contramap {original} -> {new_name}");
                    let name = stage_name(&mut inner.stage_counter.lock(), new_name.as_str());
                    inner.senders.lock().insert(
                        name.clone(),
                        StageOrAdapter::Adapter(Adapter {
                            name: name.clone(),
                            target: original,
                            transform: transform.into_inner(),
                        }),
                    );
                    StageResponse::ContramapResponse(name)
                }
            };
            *effect.lock() = Some(Right(resp));
        }
    }
}

fn now() -> Instant {
    Instant::from_tokio(tokio::time::Instant::now())
}

/// Handle to the running stages.
#[must_use = "this handle needs to be either joined or aborted"]
pub struct TokioRunning {
    inner: Arc<TokioInner>,
    termination: watch::Receiver<bool>,
}

impl TokioRunning {
    /// Abort all stage tasks of this network.
    pub fn abort(self) {
        for handle in self.inner.handles.lock().iter() {
            handle.abort();
        }
    }

    pub async fn join(self) {
        let handles = std::mem::take(&mut *self.inner.handles.lock());
        for handle in handles {
            handle.await.unwrap_or_else(|err| {
                tracing::error!("stage task failed: {:?}", err);
            });
        }
    }

    pub fn trace_buffer(&self) -> &Arc<Mutex<TraceBuffer>> {
        &self.inner.trace_buffer
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
