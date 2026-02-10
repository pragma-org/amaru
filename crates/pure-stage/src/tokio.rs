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

use crate::{
    BoxFuture, EPOCH, Effects, Instant, Name, ScheduleId, ScheduleIds, SendData, Sender,
    StageBuildRef, StageGraph, StageRef,
    adapter::{Adapter, StageOrAdapter, find_recipient},
    drop_guard::DropGuard,
    effect::{CallExtra, CallTimeout, CanSupervise, StageEffect, StageResponse, TransitionFactory},
    effect_box::EffectBox,
    resources::Resources,
    serde::NoDebug,
    simulation::Transition,
    stage_name,
    stage_ref::StageStateRef,
    stagegraph::StageGraphRunning,
    time::Clock,
    trace_buffer::TraceBuffer,
};
use amaru_observability::{stage, trace_span};
use either::Either::{Left, Right};
use futures_util::{FutureExt, StreamExt, stream::FuturesUnordered};
use parking_lot::Mutex;
use std::{
    any::Any,
    collections::BTreeMap,
    future::{Future, poll_fn},
    marker::PhantomData,
    sync::Arc,
    task::{Context, Poll, Waker},
    time::Duration,
};
use tokio::{
    runtime::Handle,
    sync::{
        mpsc::{self, Receiver},
        oneshot, watch,
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
    schedule_ids: ScheduleIds,
    mailbox_size: usize,
    stage_counter: Mutex<usize>,
    trace_buffer: Arc<Mutex<TraceBuffer>>,
}

impl TokioInner {
    fn new() -> Self {
        Self {
            senders: Default::default(),
            handles: Default::default(),
            clock: Arc::new(TokioClock),
            resources: Resources::default(),
            schedule_ids: ScheduleIds::default(),
            mailbox_size: 10,
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
    termination_tx: watch::Sender<bool>,
}

impl Default for TokioBuilder {
    fn default() -> Self {
        let (termination_tx, termination_rx) = watch::channel(false);
        Self {
            tasks: Default::default(),
            inner: TokioInner::new(),
            termination_tx,
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
            termination_tx: _, // only statically spawned stages can terminate the network
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
            let handles = inner2.handles.lock();
            tracing::info!(
                stages = handles.len(),
                "termination signal received, shutting down stages"
            );
            for handle in handles.iter() {
                handle.abort();
            }
        });

        TokioRunning { inner, termination }
    }

    pub fn with_trace_buffer(mut self, trace_buffer: Arc<Mutex<TraceBuffer>>) -> Self {
        self.inner.trace_buffer = trace_buffer;
        self
    }

    pub fn with_schedule_ids(mut self, schedule_ids: ScheduleIds) -> Self {
        self.inner.schedule_ids = schedule_ids;
        self
    }

    pub fn with_epoch_clock(mut self) -> Self {
        self.inner.clock = Arc::new(EpochClock::new());
        self
    }
}

struct EpochClock {
    offset: Mutex<Option<Duration>>,
}

impl EpochClock {
    fn new() -> Self {
        Self {
            offset: Mutex::new(None),
        }
    }
}

impl Clock for EpochClock {
    fn now(&self) -> Instant {
        let mut offset = self.offset.lock();
        if let Some(offset) = *offset {
            Instant::now() - offset
        } else {
            let now = Instant::now();
            let since_epoch = now.saturating_since(*EPOCH);
            *offset = Some(since_epoch);
            now - since_epoch
        }
    }

    fn advance_to(&self, _instant: Instant) {}
}

type RefAux = (Receiver<Box<dyn SendData>>, TransitionFactory);

impl StageGraph for TokioBuilder {
    #[expect(clippy::expect_used)]
    fn stage<Msg: SendData, St: SendData, F, Fut>(
        &mut self,
        name: impl AsRef<str>,
        mut f: F,
    ) -> StageBuildRef<Msg, St, Box<dyn Any + Send>>
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

        let me = StageRef::new(name.clone());
        let clock = self.inner.clock.clone();
        let resources = self.inner.resources.clone();
        let schedule_ids = self.inner.schedule_ids.clone();
        let trace_buffer = self.inner.trace_buffer.clone();
        let ff = Box::new(move |effect| {
            let eff = Effects::new(me, effect, clock, resources, schedule_ids, trace_buffer);
            Box::new(move |state: Box<dyn SendData>, msg: Box<dyn SendData>| {
                let state = state.cast::<St>().expect("internal state type error");
                let msg = msg.cast::<Msg>().expect("internal message type error");
                let state = f(*state, *msg, eff.clone());
                Box::pin(async move { Box::new(state.await) as Box<dyn SendData> })
                    as BoxFuture<'static, Box<dyn SendData>>
            }) as Transition
        });
        let network: RefAux = (rx, ff);

        StageBuildRef {
            name,
            network: Box::new(network),
            _ph: PhantomData,
        }
    }

    #[expect(clippy::expect_used)]
    fn wire_up<Msg: SendData, St: SendData>(
        &mut self,
        stage: StageBuildRef<Msg, St, Box<dyn Any + Send>>,
        state: St,
    ) -> StageStateRef<Msg, St> {
        let StageBuildRef { name, network, _ph } = stage;
        let (rx, ff) = *network
            .downcast::<RefAux>()
            .expect("internal network type error");
        let stage_name = name.clone();
        let state = Box::new(state);
        let termination_tx = self.termination_tx.clone();
        self.tasks.push(Box::new(move |inner| {
            let stage = run_stage_boxed(state, rx, ff, stage_name, inner);
            Box::pin(async move {
                stage.await;
                termination_tx.send_replace(true);
            })
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

enum PriorityMessage {
    Scheduled(Box<dyn SendData>, ScheduleId, watch::Receiver<bool>),
    TimerCancelled(ScheduleId),
    Tombstone(Box<dyn SendData>),
}

async fn run_stage_boxed(
    mut state: Box<dyn SendData>,
    mut rx: Receiver<Box<dyn SendData + 'static>>,
    transition: TransitionFactory,
    stage_name: Name,
    inner: Arc<TokioInner>,
) {
    tracing::debug!("running stage `{stage_name}`");

    let effect = Arc::new(Mutex::new(None));
    let mut transition = transition(effect.clone());

    // this also contains tasks tracking the termination of spawned stages, which when dropped
    // will terminate those spawned stages
    let mut timers = FuturesUnordered::<BoxFuture<'static, PriorityMessage>>::new();
    let mut cancel_senders = BTreeMap::<ScheduleId, watch::Sender<bool>>::new();

    let tb = DropGuard::new(inner.trace_buffer.clone(), |tb| {
        // ensure that Aborted is traced when this Future is dropped
        tb.lock().push_terminated_aborted(&stage_name)
    });

    let mut msgs = Vec::new();

    inner.trace_buffer.lock().push_state(&stage_name, &state);

    'outer: loop {
        let poll_timers = !timers.is_empty();
        // if multiple timers have fired since the last poll, we need them all so that we can deliver them in order
        let mut timer_chunks = (&mut timers).ready_chunks(1000);

        tokio::select! { biased;
            Some(res) = timer_chunks.next(), if poll_timers => {
                let mut scheduled = Vec::new();
                for msg in res {
                    match msg {
                        PriorityMessage::Scheduled(msg, id, cancelation) => {
                            scheduled.push((id, msg, cancelation));
                        }
                        PriorityMessage::TimerCancelled(_id) => {}
                        PriorityMessage::Tombstone(msg) => msgs.push((msg, None)),
                    }
                }
                // ensure that earliest timer is delivered first
                scheduled.sort_by_key(|(id, _, _)| *id);
                for (id, msg, cancelation) in scheduled {
                    msgs.push((msg, Some((id, cancelation))));
                }
            }
            Some(msg) = rx.recv() => msgs.push((msg, None)),
            else => {
                tracing::error!(%stage_name, "stage sender dropped");
                break;
            }
        }

        for (msg, cancelation) in msgs.drain(..) {
            if let Some((id, canceled)) = cancelation {
                cancel_senders.remove(&id);
                if *canceled.borrow() {
                    // cancellation happened after the timer fired but before the message was delivered
                    continue;
                }
            }

            if let Ok(CanSupervise(child)) = msg.cast_ref::<CanSupervise>() {
                tracing::debug!(
                    "stage `{stage_name}` terminates because of an unsupervised child termination"
                );
                tb.lock().push_terminated_supervision(&stage_name, child);
                break 'outer;
            }

            inner.trace_buffer.lock().push_input(&stage_name, &msg);

            let f = (transition)(state, msg);
            let result = interpreter(
                &inner,
                &effect,
                &stage_name,
                &mut timers,
                &mut cancel_senders,
                f,
            )
            .await;
            match result {
                Some(st) => state = st,
                None => {
                    tracing::info!(%stage_name, "terminated");
                    tb.lock().push_terminated_voluntary(&stage_name);
                    break 'outer;
                }
            }

            inner.trace_buffer.lock().push_state(&stage_name, &state);
        }
    }

    DropGuard::into_inner(tb);
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

type StageRefExtra = Mutex<Option<oneshot::Sender<Box<dyn SendData>>>>;

// clippy is lying, changing to async fn does not work.
#[expect(clippy::manual_async_fn)]
fn interpreter(
    inner: &Arc<TokioInner>,
    effect: &EffectBox,
    name: &Name,
    timers: &mut FuturesUnordered<BoxFuture<'static, PriorityMessage>>,
    cancel_senders: &mut BTreeMap<ScheduleId, watch::Sender<bool>>,
    mut stage: BoxFuture<'static, Box<dyn SendData>>,
) -> impl Future<Output = Option<Box<dyn SendData>>> + Send {
    // trying to write this as an async fn fails with inscrutable compile errors, it seems
    // that rustc has some issue with this particular pattern
    async move {
        let tb = || inner.trace_buffer.lock();
        tb().push_resume(name, &StageResponse::Unit);
        loop {
            let poll = {
                let _span = trace_span!(stage::tokio::POLL, stage = %name).entered();
                stage.as_mut().poll(&mut Context::from_waker(Waker::noop()))
            };
            if let Poll::Ready(state) = poll {
                return Some(state);
            }
            drop(poll);

            #[expect(clippy::panic)]
            let Some(Left(eff)) = effect.lock().take() else {
                panic!("stage `{name}` used .await on something that was not a stage effect");
            };
            // this does not push the Call effect because getting the message consumes it
            tb().push_suspend_ref(name, &eff);

            let resp = match eff {
                StageEffect::Receive => {
                    #[expect(clippy::panic)]
                    {
                        panic!("effect Receive cannot be explicitly awaited (stage `{name}`)")
                    }
                }
                StageEffect::Send(target, ..) if target.is_empty() => {
                    tracing::warn!(stage = %name, "message send to blackhole stage dropped");
                    StageResponse::Unit
                }
                StageEffect::Send(_target, Some(call), msg) => {
                    #[expect(clippy::expect_used)]
                    let sender = call
                        .downcast_ref::<StageRefExtra>()
                        .expect("expected CallExtra");
                    if let Some(sender) = sender.lock().take() {
                        sender.send(msg).ok();
                    }
                    StageResponse::Unit
                }
                StageEffect::Send(target, None, msg) => {
                    let (tx, msg) = {
                        let mut senders = inner.senders.lock();
                        #[expect(clippy::expect_used)]
                        let (tx, msg) = find_recipient(&mut senders, target.clone(), Some(msg))
                            .expect("stage ref contained unknown name");
                        (tx.clone(), msg)
                    };
                    tx.send(msg).await.ok();
                    StageResponse::Unit
                }
                StageEffect::Call(target, duration, msg) => {
                    #[expect(clippy::panic)]
                    let CallExtra::CallFn(NoDebug(msg)) = msg else {
                        panic!("expected CallFn, got {:?}", msg);
                    };
                    let (tx_response, rx) = oneshot::channel();
                    // it is important to use the type alias StageRefExtra here, otherwise the
                    // compiler would accept any type that implements Send + Sync + 'static
                    let sender = StageRefExtra::new(Some(tx_response));
                    let msg = (msg)(name.clone(), Arc::new(sender));

                    tb().push_suspend_call(name, &target, duration, &*msg);

                    let (tx_call, msg) = {
                        let mut senders = inner.senders.lock();
                        #[expect(clippy::expect_used)]
                        let (tx, msg) = find_recipient(&mut senders, target.clone(), Some(msg))
                            .expect("stage ref contained unknown name");
                        (tx.clone(), msg)
                    };
                    tx_call.send(msg).await.ok();
                    match tokio::time::timeout(duration, rx).await {
                        Ok(Ok(msg)) => StageResponse::CallResponse(msg),
                        _ => StageResponse::CallResponse(Box::new(CallTimeout)),
                    }
                }
                StageEffect::Clock => StageResponse::ClockResponse(now()),
                StageEffect::Wait(duration) => {
                    tokio::time::sleep(duration).await;
                    StageResponse::WaitResponse(now())
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
                StageEffect::WireStage(name, transition, initial_state, tombstone) => {
                    tracing::debug!("stage `{name}` wired");
                    let (tx, rx) = mpsc::channel(inner.mailbox_size);
                    inner
                        .senders
                        .lock()
                        .insert(name.clone(), StageOrAdapter::Stage(tx));
                    let stage = run_stage_boxed(
                        initial_state,
                        rx,
                        transition.into_inner(),
                        name.clone(),
                        inner.clone(),
                    );
                    let handle = tokio::spawn(stage);
                    // need to construct DropGuard before pushing into the FuturesUnordered to avoid Future being dropped
                    // before the guard is established
                    let mut handle = DropGuard::new(handle, |handle| handle.abort());
                    timers.push(Box::pin(async move {
                        if let Err(err) = (&mut *handle).await {
                            tracing::error!("stage `{name}` failed: {}", err);
                        }
                        PriorityMessage::Tombstone(tombstone)
                    }));
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
                StageEffect::Schedule(msg, id) => {
                    let when = id.time();
                    let sleep = tokio::time::sleep_until(when.to_tokio());
                    let (tx, mut rx) = watch::channel(false);
                    cancel_senders.insert(id, tx);
                    timers.push(Box::pin(async move {
                        let rx2 = rx.clone();
                        tokio::select! { biased;
                            _ = rx.wait_for(|x| *x) => PriorityMessage::TimerCancelled(id),
                            _ = sleep => PriorityMessage::Scheduled(msg, id, rx2),
                        }
                    }));
                    StageResponse::Unit
                }
                StageEffect::CancelSchedule(id) => {
                    if let Some(tx) = cancel_senders.remove(&id) {
                        tx.send_replace(true);
                        StageResponse::CancelScheduleResponse(true)
                    } else {
                        StageResponse::CancelScheduleResponse(false)
                    }
                }
            };
            tb().push_resume(name, &resp);
            *effect.lock() = Some(Right(resp));
        }
    }
}

fn now() -> Instant {
    Instant::from_tokio(tokio::time::Instant::now())
}

/// Handle to the running stages.
#[derive(Clone)]
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
        poll_fn(move |cx| {
            let mut handles = self.inner.handles.lock();
            handles.retain_mut(|h| {
                if let Poll::Ready(res) = h.poll_unpin(cx) {
                    match res {
                        Ok(_) => tracing::info!("stage task completed"),
                        Err(err) if err.is_cancelled() => tracing::info!("stage task cancelled"),
                        Err(err) => tracing::error!("stage task failed: {:?}", err),
                    }
                    false
                } else {
                    true
                }
            });
            if handles.is_empty() {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await;
    }

    pub fn trace_buffer(&self) -> &Arc<Mutex<TraceBuffer>> {
        &self.inner.trace_buffer
    }

    pub fn resources(&self) -> &Resources {
        &self.inner.resources
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
