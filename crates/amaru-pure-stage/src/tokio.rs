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

use std::{
    any::Any,
    collections::BTreeMap,
    future::{Future, poll_fn},
    marker::PhantomData,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll, Waker},
    time::Duration,
};

use either::Either::{Left, Right};
use futures_util::{FutureExt, StreamExt, stream::FuturesUnordered};
use parking_lot::Mutex;
use tokio::{
    runtime::Handle,
    sync::{
        mpsc::{self, Receiver},
        oneshot, watch,
    },
    task::{JoinError, JoinHandle},
};
use tracing::trace_span;

use crate::{
    BoxFuture, DEFAULT_MAILBOX_SIZE, Effects, Instant, Name, PRIORITY_MAILBOX_SIZE, ScheduleId, ScheduleIds, SendData,
    Sender, StageBuildRef, StageGraph, StageRef, TrySend,
    drop_guard::DropGuard,
    effect::{CallExtra, CallNotAdmitted, CallTimeout, CanSupervise, StageEffect, StageResponse, TransitionFactory},
    effect_box::EffectBox,
    resources::Resources,
    sender::StageRefExtra,
    serde::NoDebug,
    simulation::Transition,
    stage_name,
    stage_ref::StageStateRef,
    stagegraph::StageGraphRunning,
    time::Clock,
    timeouts::TimeoutHeap,
    trace_buffer::TraceBuffer,
};

#[derive(Debug, thiserror::Error)]
#[error("message send failed to stage `{target}`")]
pub struct SendError {
    target: Name,
}

struct TokioInner {
    senders: Mutex<BTreeMap<Name, mpsc::Sender<Box<dyn SendData>>>>,
    handles: Mutex<Vec<JoinHandle<()>>>,
    failures: Mutex<Vec<JoinError>>,
    unexpected_exits: Mutex<Vec<Name>>,
    stopping: AtomicBool,
    clock: Arc<dyn Clock + Send + Sync>,
    global_epoch_offset: Duration,
    resources: Resources,
    schedule_ids: ScheduleIds,
    mailbox_size: usize,
    priority_mailbox_size: usize,
    stage_counter: Mutex<usize>,
    trace_buffer: Arc<Mutex<TraceBuffer>>,
}

impl TokioInner {
    fn new() -> Self {
        Self {
            senders: Default::default(),
            handles: Default::default(),
            failures: Default::default(),
            unexpected_exits: Default::default(),
            stopping: AtomicBool::new(false),
            clock: Arc::new(TokioClock),
            global_epoch_offset: Duration::ZERO,
            resources: Resources::default(),
            schedule_ids: ScheduleIds::default(),
            mailbox_size: DEFAULT_MAILBOX_SIZE,
            priority_mailbox_size: PRIORITY_MAILBOX_SIZE,
            stage_counter: Mutex::new(0usize),
            trace_buffer: TraceBuffer::new_shared(0, 0),
        }
    }

    fn push_handle(&self, handle: JoinHandle<()>) {
        let mut handles = self.handles.lock();
        if self.stopping.load(Ordering::SeqCst) {
            handle.abort();
        }
        reap_finished_handles(&mut handles, &mut self.failures.lock());
        handles.push(handle);
    }

    fn request_abort(&self) {
        self.stopping.store(true, Ordering::SeqCst);
        self.handles.lock().iter().for_each(JoinHandle::abort);
    }
}

fn reap_finished_handles(handles: &mut Vec<JoinHandle<()>>, failures: &mut Vec<JoinError>) {
    handles.retain_mut(|handle| {
        if !handle.is_finished() {
            return true;
        }
        let mut cx = Context::from_waker(Waker::noop());
        match handle.poll_unpin(&mut cx) {
            Poll::Ready(Ok(())) => {}
            Poll::Ready(Err(err)) if err.is_cancelled() => {}
            Poll::Ready(Err(err)) => failures.push(err),
            Poll::Pending => return true,
        }
        false
    });
}

struct TokioClock;
impl Clock for TokioClock {
    fn now(&self, global_epoch_offset: Duration) -> Instant {
        Instant::from_tokio(tokio::time::Instant::now(), global_epoch_offset)
    }
    fn advance_to(&self, _instant: Instant) {}
}

struct PendingStage {
    mailbox_size: usize,
    tx: mpsc::Sender<Box<dyn SendData>>,
    rx: mpsc::Receiver<Box<dyn SendData>>,
    transition: TransitionFactory,
}

/// Open a bulk mailbox.
///
/// `tokio::sync::mpsc::channel(0)` panics (the buffer must be at least 1). Capacity zero is
/// still a rendezvous: the channel has one slot, and the stage task holds that slot whenever
/// it is not waiting to receive. A message is admitted only while the stage is idle, the
/// slot is empty, and no blocking sender is already waiting on it.
fn open_mailbox(size: usize) -> (mpsc::Sender<Box<dyn SendData>>, mpsc::Receiver<Box<dyn SendData>>) {
    mpsc::channel(size.max(1))
}

/// A [`StageGraph`] implementation that dispatches each stage as a task on the Tokio global pool.
///
/// *This is currently only a minimal sketch that will likely not fit the intended design.
/// It is more likely that the effect handling will be done like in the [`SimulationBuilder`](crate::simulation::SimulationBuilder)
/// implementation.*
pub struct TokioBuilder {
    tasks: Vec<Box<dyn FnOnce(Arc<TokioInner>) -> BoxFuture<'static, ()>>>,
    inner: TokioInner,
    pending: BTreeMap<Name, PendingStage>,
    termination: watch::Receiver<bool>,
    termination_tx: watch::Sender<bool>,
}

impl Default for TokioBuilder {
    fn default() -> Self {
        let (termination_tx, termination_rx) = watch::channel(false);
        Self {
            tasks: Default::default(),
            inner: TokioInner::new(),
            pending: BTreeMap::new(),
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
            pending: _,
        } = self;
        let inner = Arc::new(inner);
        let handles = tasks.into_iter().map(|t| rt.spawn(t(inner.clone()))).collect::<Vec<_>>();
        inner.handles.lock().extend(handles);

        // abort all tasks as soon as the termination signal is received
        let mut termination2 = termination.clone();
        let inner2 = inner.clone();
        let monitor = rt.spawn(async move {
            termination2.wait_for(|x| *x).await.ok();
            tracing::info!(stages = inner2.handles.lock().len(), "termination signal received, shutting down stages");
            inner2.request_abort();
        });
        inner.push_handle(monitor);

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

    pub fn with_global_epoch_offset(mut self, offset: Duration) -> Self {
        self.inner.global_epoch_offset = offset;
        self
    }

    /// Bulk mailbox capacity passed by [`StageGraph::stage`].
    ///
    /// This is the number of messages that may wait in the mailbox. The message currently
    /// being processed does not count. Defaults to [`DEFAULT_MAILBOX_SIZE`], matching
    /// [`SimulationBuilder::with_mailbox_size`](crate::simulation::SimulationBuilder::with_mailbox_size).
    /// A single stage uses [`StageGraph::stage_with_mailbox_size`].
    /// Zero is a rendezvous: a message is admitted only when the destination is already
    /// waiting and no sender is parked ahead.
    pub fn with_mailbox_size(mut self, size: usize) -> Self {
        self.inner.mailbox_size = size;
        self
    }

    /// Set the maximum number of undelivered self-scheduled messages allowed per stage.
    ///
    /// Defaults to [`PRIORITY_MAILBOX_SIZE`]. Exceeding the limit
    /// panics so schedule storms fail loudly.
    pub fn with_priority_mailbox_size(mut self, size: usize) -> Self {
        self.inner.priority_mailbox_size = size;
        self
    }
}

impl StageGraph for TokioBuilder {
    fn stage<Msg, St, F, Fut>(&mut self, name: impl AsRef<str>, f: F) -> StageBuildRef<Msg, St, Box<dyn Any + Send>>
    where
        F: FnMut(St, Msg, Effects<Msg>) -> Fut + 'static + Send,
        Fut: Future<Output = St> + 'static + Send,
        Msg: SendData + serde::de::DeserializeOwned,
        St: SendData,
    {
        self.stage_with_mailbox_size(name, f, self.inner.mailbox_size)
    }

    #[expect(clippy::expect_used)]
    fn stage_with_mailbox_size<Msg, St, F, Fut>(
        &mut self,
        name: impl AsRef<str>,
        mut f: F,
        mailbox_size: usize,
    ) -> StageBuildRef<Msg, St, Box<dyn Any + Send>>
    where
        F: FnMut(St, Msg, Effects<Msg>) -> Fut + 'static + Send,
        Fut: Future<Output = St> + 'static + Send,
        Msg: SendData + serde::de::DeserializeOwned,
        St: SendData,
    {
        // THIS MUST MATCH THE SIMULATION BUILDER
        let name = stage_name(&mut self.inner.stage_counter.lock(), name.as_ref());
        let (tx, rx) = open_mailbox(mailbox_size);
        self.inner.senders.lock().insert(name.clone(), tx.clone());

        let me = StageRef::new(name.clone());
        let clock = self.inner.clock.clone();
        let global_epoch_offset = self.inner.global_epoch_offset;
        let resources = self.inner.resources.clone();
        let schedule_ids = self.inner.schedule_ids.clone();
        let trace_buffer = self.inner.trace_buffer.clone();
        let child_mailbox = self.inner.mailbox_size;
        let ff = Box::new(move |effect| {
            let eff = Effects::new(
                me,
                effect,
                clock,
                global_epoch_offset,
                resources,
                schedule_ids,
                trace_buffer,
                child_mailbox,
            );
            Box::new(move |state: Box<dyn SendData>, msg: Box<dyn SendData>| {
                let state = state.cast::<St>().expect("internal state type error");
                let msg = msg.cast::<Msg>().expect("internal message type error");
                let state = f(*state, *msg, eff.clone());
                Box::pin(async move { Box::new(state.await) as Box<dyn SendData> })
                    as BoxFuture<'static, Box<dyn SendData>>
            }) as Transition
        });
        self.pending.insert(name.clone(), PendingStage { mailbox_size, tx, rx, transition: ff });

        StageBuildRef { name, network: Box::new(()), mailbox_size, _ph: PhantomData }
    }

    #[expect(clippy::expect_used)]
    fn wire_up<Msg: SendData, St: SendData>(
        &mut self,
        stage: StageBuildRef<Msg, St, Box<dyn Any + Send>>,
        state: St,
    ) -> StageStateRef<Msg, St> {
        let StageBuildRef { name, .. } = stage;
        let PendingStage { rx, tx, mailbox_size, transition: ff } =
            self.pending.remove(&name).expect("stage was already wired or was not created here");
        let stage_name = name.clone();
        let state = Box::new(state);
        let termination_tx = self.termination_tx.clone();
        self.tasks.push(Box::new(move |inner| {
            let stage = run_stage_boxed(state, rx, tx, mailbox_size, ff, stage_name.clone(), inner.clone());
            Box::pin(async move {
                let _termination = DropGuard::new(termination_tx, |tx| {
                    tx.send_replace(true);
                });
                stage.await;
                if !inner.stopping.load(Ordering::SeqCst) {
                    inner.unexpected_exits.lock().push(stage_name);
                }
            })
        }));
        StageStateRef::new(name)
    }

    fn preload<Msg: SendData>(
        &mut self,
        stage: impl AsRef<StageRef<Msg>>,
        messages: impl IntoIterator<Item = Msg>,
    ) -> Result<(), Box<dyn SendData>> {
        let stage = stage.as_ref();
        let senders = self.inner.senders.lock();
        for msg in messages {
            let (_name, leftover, payload) = stage.materialize_send(msg);
            if leftover.is_some() {
                return Err(Box::new("cannot preload a call-reply StageRef".to_string()));
            }
            let Some(tx) = senders.get(stage.name()) else {
                tracing::debug!(target = %stage.name(), "stage terminated");
                continue;
            };
            if let Err(err) = tx.try_send(payload) {
                tracing::warn!("message preload failed to stage `{}`", stage.name());
                return Err(err.into_inner());
            }
        }
        Ok(())
    }

    fn input<Msg: SendData>(&mut self, stage: impl AsRef<StageRef<Msg>>) -> Sender<Msg> {
        let stage = stage.as_ref();
        mk_sender(stage, &self.inner)
    }

    fn resources(&self) -> &Resources {
        &self.inner.resources
    }
}

enum PriorityMessage {
    /// Due self-scheduled message; bypasses the bulk mpsc mailbox.
    Scheduled(Box<dyn SendData>, ScheduleId, watch::Receiver<bool>),
    TimeoutFired(u64, ScheduleId),
    TimerCancelled(ScheduleId),
    Tombstone(Box<dyn SendData>),
}

// clippy is lying, changing to async fn does not work.
#[expect(clippy::manual_async_fn)]
fn run_stage_boxed(
    mut state: Box<dyn SendData>,
    mut rx: Receiver<Box<dyn SendData + 'static>>,
    tx: mpsc::Sender<Box<dyn SendData>>,
    mailbox_size: usize,
    transition: TransitionFactory,
    stage_name: Name,
    inner: Arc<TokioInner>,
) -> impl Future<Output = ()> + Send {
    // Trying to write this as an async fn encounters a bug
    // in the Rust compiler.
    //
    // cc <https://github.com/rust-lang/rust/issues/161658>.
    async move {
        tracing::debug!("running stage `{stage_name}`");

        let effect = Arc::new(Mutex::new(None));
        let mut transition = transition(effect.clone());

        // this also contains tasks tracking the termination of spawned stages, which when dropped
        // will terminate those spawned stages
        let mut timers = FuturesUnordered::<BoxFuture<'static, PriorityMessage>>::new();
        let mut cancel_senders = BTreeMap::<ScheduleId, watch::Sender<bool>>::new();
        // Armed schedules not yet consumed by receive (matches simulation `scheduled_pending`).
        let mut scheduled_pending: usize = 0;
        let mut timeouts = TimeoutHeap::default();

        let tb = DropGuard::new(inner.trace_buffer.clone(), |tb| {
            // ensure that Aborted is traced when this Future is dropped
            tb.lock().push_terminated_aborted(&stage_name)
        });

        let mut msgs = Vec::new();
        // Capacity zero: hold the only slot while this stage is not parked in `recv`, so a
        // second message cannot sit in the buffer. Dropped just before waiting, reclaimed
        // before the next transition. A blocking sender already queued on the semaphore is
        // given the slot instead (`try_reserve_owned` fails); `try_send` cannot take it.
        let mut idle_hold: Option<mpsc::OwnedPermit<Box<dyn SendData>>> = None;

        inner.trace_buffer.lock().push_state(&stage_name, &state);

        'outer: loop {
            // Messages queued while the previous transition was awaiting an effect are already
            // ingress. Taking them before `select` keeps a due schedule ahead of newer bulk mail.
            if msgs.is_empty() {
                if mailbox_size == 0 {
                    idle_hold.take();
                }
                let poll_timers = !timers.is_empty();
                // if multiple timers have fired since the last poll, we need them all so that we can deliver them in order
                let mut timer_chunks = (&mut timers).ready_chunks(1000);

                // Prefer due scheduled (priority) messages over bulk mailbox traffic.
                tokio::select! { biased;
                    Some(res) = timer_chunks.next(), if poll_timers => {
                        collect_ingress(res, &mut cancel_senders, &mut timeouts, &mut msgs);
                    }
                    Some(msg) = rx.recv() => msgs.push((msg, false)),
                    else => {
                        tracing::error!(%stage_name, "stage sender dropped");
                        break;
                    }
                }
                if mailbox_size == 0 {
                    idle_hold = tx.clone().try_reserve_owned().ok();
                }
            }

            let batch = std::mem::take(&mut msgs);
            for (msg, release_budget) in batch {
                if release_budget {
                    scheduled_pending = scheduled_pending.saturating_sub(1);
                }

                if let Ok(CanSupervise(child)) = msg.cast_ref::<CanSupervise>() {
                    tracing::debug!("stage `{stage_name}` terminates because of an unsupervised child termination");
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
                    &mut scheduled_pending,
                    &mut timeouts,
                    &mut msgs,
                    f,
                )
                .await;
                tokio_rearm_timeouts(&inner, &mut timeouts, &mut timers, &stage_name);
                match result {
                    Some(st) => state = st,
                    None => {
                        tracing::debug!(%stage_name, "terminated");
                        tb.lock().push_terminated_voluntary(&stage_name);
                        break 'outer;
                    }
                }

                inner.trace_buffer.lock().push_state(&stage_name, &state);
            }
        }

        DropGuard::into_inner(tb);
    }
}

#[expect(clippy::expect_used, clippy::panic)]
fn mk_sender<Msg: SendData>(stage: &StageRef<Msg>, inner: &TokioInner) -> Sender<Msg> {
    let senders = inner.senders.lock();
    let tx = senders.get(stage.name()).expect("stage ref contained unknown name").clone();
    let peeled = stage.peel();
    if peeled.leftover.is_some() {
        panic!("cannot input() a call-reply StageRef");
    }
    let transform = peeled.transform;
    let target = peeled.name;
    Sender::new(Arc::new(move |msg: Msg| {
        let tx = tx.clone();
        let transform = transform.clone();
        let target = target.clone();
        let payload = match transform {
            Some(transform) => transform(Box::new(msg)),
            None => Box::new(msg),
        };
        Box::pin(async move { tx.send(payload).await.map_err(|_| crate::SendError::new(target)) })
    }))
}

fn tokio_rearm_timeouts(
    inner: &TokioInner,
    timeouts: &mut TimeoutHeap,
    timers: &mut FuturesUnordered<BoxFuture<'static, PriorityMessage>>,
    _name: &Name,
) {
    if timeouts.has_due() || timeouts.armed_is_current_min() {
        return;
    }
    let _ = timeouts.armed.take();
    let Some((slot, when)) = timeouts.min_slot() else {
        return;
    };
    let id = inner.schedule_ids.next_at(when);
    timeouts.armed = Some((id, slot));
    let sleep = tokio::time::sleep_until(when.to_tokio());
    timers.push(Box::pin(async move {
        sleep.await;
        PriorityMessage::TimeoutFired(slot, id)
    }));
}

/// Move due priority-path messages into `msgs`.
///
/// A schedule is ingress at this point, including when the stage is inside `Wait`, `Call`,
/// `Send`, or an external effect. That matches simulation `deliver_priority`. Cancel after
/// this returns false and the message is still delivered. The budget is released when the
/// message is received, unless cancel already released it.
fn collect_ingress(
    chunk: Vec<PriorityMessage>,
    cancel_senders: &mut BTreeMap<ScheduleId, watch::Sender<bool>>,
    timeouts: &mut TimeoutHeap,
    msgs: &mut Vec<(Box<dyn SendData>, bool)>,
) {
    let mut scheduled = Vec::new();
    for msg in chunk {
        match msg {
            PriorityMessage::Scheduled(msg, id, cancellation) => {
                cancel_senders.remove(&id);
                if !*cancellation.borrow() {
                    scheduled.push((id, msg));
                }
            }
            PriorityMessage::TimeoutFired(slot, id) => {
                if timeouts.fire(slot, id)
                    && let Some(msg) = timeouts.take_due()
                {
                    msgs.push((msg, false));
                }
            }
            PriorityMessage::TimerCancelled(_id) => {}
            PriorityMessage::Tombstone(msg) => msgs.push((msg, false)),
        }
    }
    // ensure that earliest timer is delivered first
    scheduled.sort_by_key(|(id, _)| *id);
    for (_id, msg) in scheduled {
        msgs.push((msg, true));
    }
}

/// Poll `effect`, and also the priority timer set, until `effect` completes.
///
/// Due timers are taken first when both are ready, so a paused clock that jumps across
/// several deadlines still records the schedule as ingress before the effect returns.
async fn poll_with_ingress<T>(
    effect: impl Future<Output = T>,
    timers: &mut FuturesUnordered<BoxFuture<'static, PriorityMessage>>,
    cancel_senders: &mut BTreeMap<ScheduleId, watch::Sender<bool>>,
    timeouts: &mut TimeoutHeap,
    msgs: &mut Vec<(Box<dyn SendData>, bool)>,
) -> T {
    let mut effect = std::pin::pin!(effect);
    loop {
        let poll_timers = !timers.is_empty();
        let mut timer_chunks = (&mut *timers).ready_chunks(1000);
        tokio::select! { biased;
            Some(chunk) = timer_chunks.next(), if poll_timers => {
                collect_ingress(chunk, cancel_senders, timeouts, msgs);
            }
            result = &mut effect => return result,
        }
    }
}

/// Wait out one call deadline, covering enqueue and the reply.
///
/// `send` is cancel-safe: if the deadline wins before the message is queued, dropping the
/// send future leaves the mailbox unchanged. A closed mailbox or a dropped reply does not
/// finish the call early; the caller stays suspended until the deadline.
async fn await_call(
    tx: Option<mpsc::Sender<Box<dyn SendData>>>,
    msg: Box<dyn SendData>,
    rx: oneshot::Receiver<Box<dyn SendData>>,
    duration: Duration,
) -> StageResponse {
    let deadline = tokio::time::Instant::now() + duration;
    // `timeout_at` on `send` drops the send future when the deadline fires, so a request
    // that has not been admitted is never delivered later. A send that already completed
    // stays in the mailbox; the deadline does not pull it back out.
    let response = match tx {
        Some(tx) => match tokio::time::timeout_at(deadline, tx.send(msg)).await {
            Ok(Ok(())) => match reply_until(deadline, rx).await {
                Some(msg) => msg,
                // Admitted, then the deadline passed: `CallAdmission::TimedOut`. The request stays queued.
                None => CallTimeout::boxed(),
            },
            Ok(Err(_)) => {
                tokio::time::sleep_until(deadline).await;
                CallNotAdmitted::boxed()
            }
            // Deadline fired before admission: `CallAdmission::NotAdmitted`. The request is never delivered.
            Err(_) => CallNotAdmitted::boxed(),
        },
        None => {
            tokio::time::sleep_until(deadline).await;
            CallNotAdmitted::boxed()
        }
    };
    StageResponse::CallResponse(response)
}

async fn reply_until(
    deadline: tokio::time::Instant,
    rx: oneshot::Receiver<Box<dyn SendData>>,
) -> Option<Box<dyn SendData>> {
    match tokio::time::timeout_at(deadline, rx).await {
        Ok(Ok(msg)) => Some(msg),
        Ok(Err(_)) => {
            tokio::time::sleep_until(deadline).await;
            None
        }
        Err(_) => None,
    }
}

#[expect(clippy::too_many_arguments)]
async fn interpreter(
    inner: &Arc<TokioInner>,
    effect: &EffectBox,
    name: &Name,
    timers: &mut FuturesUnordered<BoxFuture<'static, PriorityMessage>>,
    cancel_senders: &mut BTreeMap<ScheduleId, watch::Sender<bool>>,
    scheduled_pending: &mut usize,
    timeouts: &mut TimeoutHeap,
    msgs: &mut Vec<(Box<dyn SendData>, bool)>,
    mut stage: BoxFuture<'static, Box<dyn SendData>>,
) -> Option<Box<dyn SendData>> {
    let mut last_yield = tokio::time::Instant::now();
    let tb = || inner.trace_buffer.lock();
    tb().push_resume(name, &StageResponse::Unit);
    loop {
        let poll = {
            let _span = trace_span!("amaru::stages::interpreter::tokio::POLL", stage = %name).entered();
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
                let sender = call.downcast_ref::<StageRefExtra>().expect("expected CallExtra");
                if let Some(sender) = sender.lock().take() {
                    sender.send(msg).ok();
                }
                StageResponse::Unit
            }
            StageEffect::Send(target, None, msg) => {
                let tx = {
                    let senders = inner.senders.lock();
                    #[expect(clippy::expect_used)]
                    senders.get(&target).expect("stage ref contained unknown name").clone()
                };
                poll_with_ingress(tx.send(msg), timers, cancel_senders, timeouts, msgs).await.ok();
                StageResponse::Unit
            }
            StageEffect::TrySend(target, msg) => {
                // `None` is a blackhole (dropped, reported as queued). `Some` is a real mailbox.
                enum Slot {
                    Blackhole,
                    Missing,
                    Reserved(mpsc::OwnedPermit<Box<dyn SendData>>),
                    Full,
                    Closed,
                }
                let slot = if target.is_empty() {
                    tracing::warn!(stage = %name, "try_send to blackhole stage dropped");
                    Slot::Blackhole
                } else {
                    match inner.senders.lock().get(&target).cloned() {
                        None => Slot::Missing,
                        Some(tx) => match tx.try_reserve_owned() {
                            Ok(permit) => Slot::Reserved(permit),
                            Err(mpsc::error::TrySendError::Full(_)) => Slot::Full,
                            Err(mpsc::error::TrySendError::Closed(_)) => Slot::Closed,
                        },
                    }
                };
                let outcome = match &slot {
                    Slot::Blackhole | Slot::Reserved(_) => TrySend::Queued,
                    Slot::Full => TrySend::Full,
                    Slot::Missing | Slot::Closed => TrySend::Gone,
                };
                if let Slot::Reserved(permit) = slot {
                    permit.send(msg);
                }
                StageResponse::TrySend(outcome)
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

                let tx_call = if target.is_empty() { None } else { inner.senders.lock().get(&target).cloned() };
                poll_with_ingress(await_call(tx_call, msg, rx, duration), timers, cancel_senders, timeouts, msgs).await
            }
            StageEffect::Clock => StageResponse::ClockResponse(inner.clock.now(inner.global_epoch_offset)),
            StageEffect::Wait(duration) => {
                poll_with_ingress(tokio::time::sleep(duration), timers, cancel_senders, timeouts, msgs).await;
                StageResponse::WaitResponse(inner.clock.now(inner.global_epoch_offset))
            }
            StageEffect::External(effect) => {
                tracing::debug!("stage `{name}` external effect: {:?}", effect);
                // Many external effects are wrap_sync / immediately ready. Without an explicit
                // yield, a stage can process thousands of them in a single task poll and ignore
                // JoinHandle::abort until the whole transition finishes.
                let now = tokio::time::Instant::now();
                if now.duration_since(last_yield) > Duration::from_millis(100) {
                    last_yield = now;
                    tokio::task::yield_now().await;
                }
                let response =
                    poll_with_ingress(effect.run(inner.resources.clone()), timers, cancel_senders, timeouts, msgs)
                        .await;
                StageResponse::ExternalResponse(response)
            }
            StageEffect::Detach(effect, inject) => {
                tracing::debug!("stage `{name}` detach effect: {:?}", effect);
                let now = tokio::time::Instant::now();
                if now.duration_since(last_yield) > Duration::from_millis(100) {
                    last_yield = now;
                    tokio::task::yield_now().await;
                }
                let resources = inner.resources.clone();
                let target = name.clone();
                let inject = inject.into_inner();
                let inner2 = inner.clone();
                let handle = tokio::spawn(async move {
                    let result = effect.run(resources).await;
                    let msg = inject(result);
                    let tx = inner2.senders.lock().get(&target).cloned();
                    if let Some(tx) = tx {
                        if tx.send(msg).await.is_err() {
                            tracing::debug!(stage = %target, "detach result dropped: stage gone");
                        }
                    } else {
                        tracing::debug!(stage = %target, "detach result dropped: unknown stage");
                    }
                });
                inner.push_handle(handle);
                StageResponse::ExternalResponse(Box::new(()))
            }
            StageEffect::Terminate => {
                tracing::debug!("stage `{name}` terminated");
                return None;
            }
            StageEffect::AddStage(name) => {
                tracing::debug!("stage `{name}` added");
                let name = stage_name(&mut inner.stage_counter.lock(), name.as_str());
                StageResponse::AddStageResponse(name)
            }
            StageEffect::WireStage(name, transition, initial_state, tombstone, mailbox_size) => {
                tracing::debug!("stage `{name}` wired");
                let (tx, rx) = open_mailbox(mailbox_size);
                inner.senders.lock().insert(name.clone(), tx.clone());
                let stage = run_stage_boxed(
                    initial_state,
                    rx,
                    tx,
                    mailbox_size,
                    transition.into_inner(),
                    name.clone(),
                    inner.clone(),
                );
                let (done_tx, done_rx) = oneshot::channel();
                let handle = tokio::spawn(async move {
                    stage.await;
                    let _ = done_tx.send(());
                });
                let abort = DropGuard::new(handle.abort_handle(), |handle| handle.abort());
                inner.push_handle(handle);
                timers.push(Box::pin(async move {
                    let _abort = abort;
                    let _ = done_rx.await;
                    PriorityMessage::Tombstone(tombstone)
                }));
                StageResponse::Unit
            }
            StageEffect::Schedule(msg, id) => {
                let limit = inner.priority_mailbox_size;
                #[expect(clippy::panic)]
                if *scheduled_pending >= limit {
                    panic!(
                        "stage `{name}` exceeded priority mailbox size ({limit}): too many outstanding scheduled messages"
                    );
                }
                *scheduled_pending += 1;
                let when = id.time();
                let sleep = tokio::time::sleep_until(when.to_tokio());
                let (tx, mut rx) = watch::channel(false);
                cancel_senders.insert(id, tx);
                // Priority path: timers bypass the bounded mpsc bulk mailbox.
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
                    // Free the slot before this transition continues, as the simulation does.
                    // TimerCancelled must not decrement again.
                    *scheduled_pending = scheduled_pending.saturating_sub(1);
                    StageResponse::CancelScheduleResponse(true)
                } else {
                    StageResponse::CancelScheduleResponse(false)
                }
            }
            StageEffect::SetTimeout { slot, delay, msg } => {
                let now = inner.clock.now(inner.global_epoch_offset);
                timeouts.set(slot, now + delay, msg);
                tokio_rearm_timeouts(inner, timeouts, timers, name);
                StageResponse::Unit
            }
            StageEffect::ClearTimeout { slot } => {
                timeouts.clear(slot);
                tokio_rearm_timeouts(inner, timeouts, timers, name);
                StageResponse::Unit
            }
        };
        tb().push_resume(name, &resp);
        *effect.lock() = Some(Right(resp));
    }
}

/// Normal root-stage exits observed before the graph was asked to stop.
///
/// Dynamically spawned stages and detached effects may complete normally and are not included.
#[derive(Debug, Default, Clone, Eq, PartialEq)]
#[must_use = "inspect unexpected_exits even when no task panicked"]
pub struct TokioJoinReport {
    pub unexpected_exits: Vec<Name>,
}

/// Handle to the running stages.
#[derive(Clone)]
#[must_use = "this handle needs to be either joined or aborted"]
pub struct TokioRunning {
    inner: Arc<TokioInner>,
    termination: watch::Receiver<bool>,
}

impl TokioRunning {
    /// Abort all stage tasks of this network without consuming the handle.
    ///
    /// Safe to call from any thread (including the process main thread). Abort is
    /// cooperative: stage tasks stop at their next `.await`.
    pub fn request_abort(&self) {
        self.inner.request_abort();
    }

    /// Return an abort callback that does not keep the graph or its resources alive.
    pub fn abort_callback(&self) -> impl Fn() + Send + Sync + 'static {
        let inner = Arc::downgrade(&self.inner);
        move || {
            if let Some(inner) = inner.upgrade() {
                inner.request_abort();
            }
        }
    }

    /// Abort all stage tasks of this network.
    pub fn abort(self) {
        self.request_abort();
    }

    /// Wait for all registered tasks, reporting early root-stage exits or a task panic.
    ///
    /// `Ok` means no task panicked; inspect the report for roots that exited before shutdown.
    /// Requesting an abort after a root has exited does not erase that exit from the report.
    pub async fn join(self) -> Result<TokioJoinReport, JoinError> {
        poll_fn(|cx| {
            let mut handles = self.inner.handles.lock();
            handles.retain_mut(|h| {
                if let Poll::Ready(res) = h.poll_unpin(cx) {
                    match res {
                        Ok(_) => tracing::info!("stage task completed"),
                        Err(err) if err.is_cancelled() => tracing::info!("stage task cancelled"),
                        Err(err) => self.inner.failures.lock().push(err),
                    }
                    false
                } else {
                    true
                }
            });
            if handles.is_empty() { Poll::Ready(()) } else { Poll::Pending }
        })
        .await;

        if let Some(error) = self.inner.failures.lock().drain(..).next() {
            return Err(error);
        }
        Ok(TokioJoinReport { unexpected_exits: std::mem::take(&mut *self.inner.unexpected_exits.lock()) })
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
