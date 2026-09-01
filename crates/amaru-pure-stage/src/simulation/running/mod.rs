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

#![expect(clippy::wildcard_enum_match_arm, clippy::unwrap_used, clippy::panic, clippy::expect_used)]

use std::{
    collections::{BTreeMap, VecDeque},
    mem::replace,
    sync::Arc,
    task::{Context, Poll, Waker},
    time::Duration,
};

use either::Either::{Left, Right};
use override_external_effect::OverrideExternalEffect;
use parking_lot::Mutex;
use rand::rngs::StdRng;
use tokio::{runtime::Handle, select, sync::watch};

use crate::{
    BLACKHOLE_NAME, BoxFuture, DurationDist, Effect, ExternalEffect, ExternalEffectAPI, Instant, Name, Resources,
    ScheduleId, SendData, StageRef, StageResponse,
    effect::{CallExtra, CanSupervise, InjectFn, ScheduleIds, StageEffect},
    effect_box::EffectBox,
    simulation::{
        blocked::{Blocked, SendBlock},
        inputs::Inputs,
        random::EvalStrategy,
        running::{
            resume::{
                resume_add_stage_internal, resume_call_internal, resume_call_send_internal,
                resume_cancel_schedule_internal, resume_clock_internal, resume_detach_internal,
                resume_external_internal, resume_receive_internal, resume_schedule_internal, resume_send_internal,
                resume_wait_internal, resume_wire_stage_internal,
            },
            scheduled_runnables::ScheduledRunnables,
        },
        state::{StageData, StageState},
    },
    stage_name,
    stage_ref::StageStateRef,
    stagegraph::StageGraphRunning,
    time::Clock,
    trace_buffer::{TerminationReason, TraceBuffer},
};

mod resume;
mod scheduled_runnables;

/// A handle to a running [`crate::simulation::SimulationBuilder`].
///
/// It allows fine-grained control over single-stepping the simulation and when each
/// stage effect is resumed (using [`Self::try_effect`] and [`Self::handle_effect`],
/// respectively). This means that any interleaving of computations can be exercised.
/// Where this is not needed, you use [`Self::run_until_blocked`] to automate the
/// sending and receiving of messages within the simulated processing network.
///
/// Note that all stages start out in the state of waiting to receive their first message,
/// so you need to use [`resume_receive`](Self::resume_receive) to get them running.
/// See also [`run_until_blocked`](Self::run_until_blocked) for how to achieve this.
pub struct SimulationRunning {
    stages: BTreeMap<Name, StageData>,
    stage_count: usize,
    inputs: Inputs,
    effect: EffectBox,
    clock: Arc<dyn Clock + Send + Sync>,
    global_epoch_offset: Duration,
    resources: Resources,
    runnable: VecDeque<(Name, StageResponse)>,
    scheduled: ScheduledRunnables,
    mailbox_size: usize,
    priority_mailbox_size: usize,
    overrides: Vec<OverrideExternalEffect>,
    breakpoints: Vec<(Name, Box<dyn Fn(&Effect) -> bool + Send + 'static>)>,
    schedule_ids: ScheduleIds,
    trace_buffer: Arc<Mutex<TraceBuffer>>,
    eval_strategy: Box<dyn EvalStrategy>,
    duration_rng: StdRng,
    terminate: watch::Sender<bool>,
    termination: watch::Receiver<bool>,
    /// `ExternalEffect::run` futures, keyed by stage. Timed effects are forced at their deadline.
    pending_computations: BTreeMap<Name, BoxFuture<'static, Box<dyn SendData>>>,
    /// In-flight external effects: `δ` is scheduled when the effect is issued; the result may arrive later.
    external_inflight: BTreeMap<Name, PendingExternal>,
    /// Detached `run()` futures, keyed independently of the stage (many may be in flight).
    pending_detach_computations: BTreeMap<u64, BoxFuture<'static, Box<dyn SendData>>>,
    /// In-flight detaches: `δ` delays mailbox delivery; the stage is not parked on the airlock.
    detach_inflight: BTreeMap<u64, PendingDetach>,
    next_detach_id: u64,
    /// Detach results that could not yet be delivered because the bulk mailbox was full.
    undelivered_detaches: VecDeque<(Name, Box<dyn SendData>)>,
    /// Drives still-pending `run()` futures to completion when a sampled deadline fires.
    tokio_handle: Handle,
    /// When true, AddStage/WireStage effects (dynamic child stage creation via `eff.stage` + `eff.wire_up`)
    /// succeed for the parent (responses are delivered, effects traced) but no real child stage is
    /// materialized in the simulation. Subsequent sends to the child's StageRef are dropped (NotFound).
    /// This is intended for testing parent stage orchestration logic without having to implement
    /// or override effects for the child stages.
    virtual_child_stages: bool,
}

/// An external effect whose `δ` was fixed when the effect was issued.
struct PendingExternal {
    result: Option<Box<dyn SendData>>,
    /// `true` once the sampled deadline has been reached, or the dist has no `δ`
    /// ([`DurationDist::Zero`] or [`DurationDist::UntilResolved`]).
    time_ready: bool,
    /// When time is ready and no result is stored, poll/`block_on` the scheduled `run()`.
    /// `false` for [`DurationDist::UntilResolved`]: the world runner owns completion.
    force_on_ready: bool,
    dist: DurationDist,
}

enum ReadyComputation {
    Blocking((Name, Box<dyn SendData>)),
    Detach((u64, Box<dyn SendData>)),
}

/// A detached external effect whose `δ` delays mailbox delivery, not the airlock ack.
struct PendingDetach {
    at_stage: Name,
    inject: Option<InjectFn>,
    result: Option<Box<dyn SendData>>,
    time_ready: bool,
    force_on_ready: bool,
    dist: DurationDist,
}

impl SimulationRunning {
    #[expect(clippy::too_many_arguments)]
    pub(super) fn new(
        stages: BTreeMap<Name, StageData>,
        inputs: Inputs,
        effect: EffectBox,
        clock: Arc<dyn Clock + Send + Sync>,
        resources: Resources,
        mailbox_size: usize,
        priority_mailbox_size: usize,
        schedule_ids: ScheduleIds,
        trace_buffer: Arc<Mutex<TraceBuffer>>,
        eval_strategy: Box<dyn EvalStrategy>,
        global_epoch_offset: Duration,
        duration_rng: StdRng,
        tokio_handle: Handle,
    ) -> Self {
        let (terminate, termination) = watch::channel(false);
        Self {
            stage_count: stages.len(),
            stages,
            inputs,
            effect,
            clock,
            global_epoch_offset,
            resources,
            runnable: VecDeque::new(),
            scheduled: ScheduledRunnables::new(),
            mailbox_size,
            priority_mailbox_size,
            overrides: Vec::new(),
            breakpoints: Vec::new(),
            schedule_ids,
            trace_buffer,
            eval_strategy,
            duration_rng,
            terminate,
            termination,
            pending_computations: BTreeMap::new(),
            external_inflight: BTreeMap::new(),
            pending_detach_computations: BTreeMap::new(),
            detach_inflight: BTreeMap::new(),
            next_detach_id: 0,
            undelivered_detaches: VecDeque::new(),
            tokio_handle,
            virtual_child_stages: false,
        }
    }

    /// Get the resources collection for the network.
    ///
    /// This can be used during tests to modify the available resources at specific points in time.
    pub fn resources(&self) -> &Resources {
        &self.resources
    }

    pub fn trace_buffer(&self) -> &Arc<Mutex<TraceBuffer>> {
        &self.trace_buffer
    }

    /// Return true if some stages are runnable.
    pub fn has_runnable(&self) -> bool {
        !self.runnable.is_empty()
    }

    /// False after the stage has terminated (including a supervised child aborted by its parent).
    pub fn contains_stage(&self, name: impl AsRef<Name>) -> bool {
        self.stages.contains_key(name.as_ref())
    }

    /// Return true if there are any effects to be run.
    pub fn has_effects(&self) -> bool {
        !self.pending_computations.is_empty() || !self.pending_detach_computations.is_empty()
    }

    /// Install a breakpoint that will be hit when an effect matching the given predicate is encountered.
    pub fn breakpoint(&mut self, name: impl AsRef<str>, predicate: impl Fn(&Effect) -> bool + Send + 'static) {
        self.breakpoints.push((Name::from(name.as_ref()), Box::new(predicate)));
    }

    /// Remove all breakpoints.
    pub fn clear_breakpoints(&mut self) {
        self.breakpoints.clear();
    }

    /// Remove the breakpoint with the given name.
    pub fn clear_breakpoint(&mut self, name: impl AsRef<str>) {
        self.breakpoints.retain(|(n, _)| n.as_str() != name.as_ref());
    }

    /// Install an override for the given external effect type.
    ///
    /// The `remaining` parameter is the number of times the override will be applied
    /// (use `usize::MAX` to apply the override indefinitely).
    /// When the override is applied, the `transform` function is called with the effect
    /// and the result is used to possibly replace the effect.
    ///
    /// If the override result is [`OverrideResult::no_match`], the effect is passed to overrides
    /// installed later than this one.
    pub fn override_external_effect<T: ExternalEffectAPI>(
        &mut self,
        remaining: usize,
        mut transform: impl FnMut(Box<T>) -> OverrideResult<T> + Send + 'static,
    ) {
        self.overrides.push(OverrideExternalEffect::new(
            remaining,
            Box::new(move |effect| {
                use override_external_effect::OverrideResult::*;
                if effect.is::<T>() {
                    // if this casting turns out to be a significant cost, we can split the
                    // overrides by TypeId and run each in an appropriately typed closure
                    #[expect(clippy::expect_used)]
                    match transform(effect.cast::<T>().expect("checked above")).0 {
                        NoMatch(effect) => NoMatch(effect as Box<dyn ExternalEffect>),
                        Handled(msg) => Handled(msg),
                        Replaced(effect) => Replaced(effect),
                    }
                } else {
                    NoMatch(effect)
                }
            }),
        ));
    }

    /// Enables or disables "virtual child stages" mode for dynamic stage creation.
    ///
    /// When enabled, any `AddStage` / `WireStage` effects (originating from a parent stage
    /// calling the `Effects::stage(...)` and `Effects::wire_up(...)` APIs) will be handled such that:
    ///
    /// - The parent stage receives the successful responses (`AddStageResponse(name)` then `Unit`).
    /// - The corresponding `Effect::AddStage` / `Effect::WireStage` entries (and their matching
    ///   `StageResponse` resumes) are recorded in the trace buffer.
    /// - The would-be child's initial state is still pushed to the trace (`push_state`).
    /// - **No actual child `StageData` is inserted** into the running simulation.
    ///
    /// Any later `Send` or `Call` from the parent (or anyone) targeting the `StageRef` returned
    /// by the virtual `wire_up` will be treated as delivery to a non-existent stage: the message
    /// is dropped and the sender is resumed as if the send had been accepted
    /// (see `DeliverMessageResult::NotFound` handling).
    ///
    /// This mode is intended to allow testing of a parent stage's logic that involves spawning
    /// child stages (e.g. supervision, initialization hand-off, regulating peers via a helper
    /// stage) without requiring the child stage's full implementation or external-effect
    /// overrides. The parent's orchestration, the names it chooses, the messages it sends to
    /// the (virtual) child, and any subsequent behavior of the parent can still be asserted
    /// via the trace and log capture.
    ///
    /// The mode can be toggled at any time; it primarily affects processing inside
    /// [`Self::handle_effect`] (and therefore the `run_until_blocked*` family).
    pub fn use_virtual_child_stages(&mut self, enabled: bool) {
        self.virtual_child_stages = enabled;
    }

    /// Get the current simulation time (with any configured global epoch offset baked in,
    /// so that `.duration_since_global_epoch()` is meaningful).
    pub fn now(&self) -> Instant {
        self.clock.now(self.global_epoch_offset)
    }

    #[cfg(test)]
    pub fn advance_clock_to(&mut self, t: Instant) {
        self.clock.advance_to(t);
        // do not push trace here; the time change will be observed on next clock() sample
    }

    /// Advance the clock to the next wakeup time.
    ///
    /// Returns `true` if wakeups were performed, `false` if there are no more wakeups or
    /// the clock was advanced to the given `max_time`.
    pub fn skip_to_next_wakeup(&mut self, mut max_time: Option<Instant>) -> bool {
        // Get the runnables that can be woken up until max_time (everything if None)
        // and run them.
        // The last wakeup time becomes the new simulation time.
        let mut tasks_run = false;
        while let Some((t, r)) = self.scheduled.wakeup(max_time) {
            if self.clock.now(self.global_epoch_offset) < t {
                self.clock.advance_to(t);
                self.trace_buffer.lock().push_clock(t);
            }
            // limit further wakeups to the same time, i.e. the clock only advances once within this method
            max_time = Some(t);
            r(self);
            tasks_run = true;
        }

        if !tasks_run && let Some(t) = max_time {
            self.clock.advance_to(t);
            self.trace_buffer.lock().push_clock(t);
        }

        tasks_run
    }

    pub fn next_wakeup(&self) -> Option<Instant> {
        self.scheduled.next_wakeup_time()
    }

    fn schedule_wakeup(&mut self, id: ScheduleId, wakeup: impl FnOnce(&mut SimulationRunning) + Send + 'static) {
        self.scheduled.schedule(id, Box::new(wakeup));
    }

    /// Record an external effect at the moment it is issued: sample `δ` now and enqueue
    /// the wakeup. The result may arrive later; the stage resumes only when both are ready.
    ///
    /// [`DurationDist::UntilResolved`] and a sampled `δ` of zero schedule nothing: completion
    /// is the result (the Future, or a manual / override resume).
    fn begin_external(&mut self, at_stage: Name, dist: DurationDist) {
        if self.external_inflight.contains_key(&at_stage) {
            return;
        }
        let delta = dist.sample(&mut self.duration_rng);
        // UntilResolved has no δ: the world runner completes the Future. Sampled δ
        // (including zero) means the computation is scheduled and must be forced when due.
        let force_on_ready = delta.is_some();
        let time_ready = match delta {
            None => true,
            Some(d) if d.is_zero() => true,
            Some(delta) => {
                let now = self.clock.now(self.global_epoch_offset);
                let id = self.schedule_ids.next_at(now + delta);
                let name = at_stage.clone();
                self.schedule_wakeup(id, move |sim| {
                    if let Some(pending) = sim.external_inflight.get_mut(&name) {
                        pending.time_ready = true;
                    }
                    sim.try_deliver_external(&name);
                });
                false
            }
        };
        self.external_inflight.insert(at_stage, PendingExternal { result: None, time_ready, force_on_ready, dist });
    }

    /// Sample `δ` for a detach: the airlock is already free; `δ` delays mailbox delivery.
    fn begin_detach(&mut self, at_stage: Name, dist: DurationDist) -> u64 {
        let id = self.next_detach_id;
        self.next_detach_id += 1;
        let delta = dist.sample(&mut self.duration_rng);
        let force_on_ready = delta.is_some();
        let time_ready = match delta {
            None => true,
            Some(d) if d.is_zero() => true,
            Some(delta) => {
                let now = self.clock.now(self.global_epoch_offset);
                let schedule_id = self.schedule_ids.next_at(now + delta);
                self.schedule_wakeup(schedule_id, move |sim| {
                    if let Some(pending) = sim.detach_inflight.get_mut(&id) {
                        pending.time_ready = true;
                    }
                    sim.try_deliver_detach(id);
                });
                false
            }
        };
        self.detach_inflight
            .insert(id, PendingDetach { at_stage, inject: None, result: None, time_ready, force_on_ready, dist });
        id
    }

    /// Offer the computed result of an in-flight external effect. Resumes the stage if
    /// the scheduled time has already been reached (or there was no `δ`).
    fn provide_external_result(&mut self, at_stage: Name, result: Box<dyn SendData>) {
        let pending = self.external_inflight.entry(at_stage.clone()).or_insert(PendingExternal {
            result: None,
            // No `begin_external` (manual resume without going through `try_effect`): treat as UntilResolved.
            time_ready: true,
            force_on_ready: false,
            dist: DurationDist::UntilResolved,
        });
        pending.result = Some(result);
        self.try_deliver_external(&at_stage);
    }

    /// Drive `run()` to completion now. Called only when a sampled deadline has been reached
    /// (or `δ` was zero), so wall-clock compute cannot slip past other simulated events.
    ///
    /// If the future is not already ready, `block_on` is bounded by
    /// [`DurationDist::force_timeout`] (`1.5 × max + 1s`). Exceeding that is a bug.
    fn force_scheduled_computation(&mut self, at_stage: &Name) {
        let Some(mut fut) = self.pending_computations.remove(at_stage) else {
            return;
        };
        let timeout = self
            .external_inflight
            .get(at_stage)
            .and_then(|pending| pending.dist.force_timeout())
            .expect("force only for sampled DurationDist");
        // Poll first so already-ready `run()` (typical wrap_sync) never enters the runtime.
        // `timeout` must be constructed inside `block_on`: current-thread test runtimes
        // have no ambient reactor, and `tokio::time::timeout` requires one.
        let result = match std::future::Future::poll(fut.as_mut(), &mut Context::from_waker(Waker::noop())) {
            Poll::Ready(result) => result,
            Poll::Pending => match self.tokio_handle.block_on(async { tokio::time::timeout(timeout, fut).await }) {
                Ok(result) => result,
                Err(_elapsed) => {
                    panic!("external effect on `{at_stage}` exceeded force timeout {timeout:?}")
                }
            },
        };
        self.provide_external_result(at_stage.clone(), result);
    }

    fn poll_ready_computations<K: Clone + Ord>(
        pending: &mut BTreeMap<K, BoxFuture<'static, Box<dyn SendData>>>,
        cx: &mut Context<'_>,
    ) -> Option<(K, Box<dyn SendData>)> {
        let keys: Vec<K> = pending.keys().cloned().collect();
        for key in keys {
            let Some(fut) = pending.get_mut(&key) else {
                continue;
            };
            match std::future::Future::poll(fut.as_mut(), cx) {
                Poll::Ready(result) => {
                    pending.remove(&key);
                    return Some((key, result));
                }
                Poll::Pending => {}
            }
        }
        None
    }

    fn provide_detach_result(&mut self, id: u64, raw: Box<dyn SendData>) {
        let Some(pending) = self.detach_inflight.get_mut(&id) else {
            tracing::debug!(id, "detach result ignored: stage already terminated");
            return;
        };
        let inject = pending.inject.take().expect("detach inject installed before run()");
        pending.result = Some(inject(raw));
        self.try_deliver_detach(id);
    }

    fn force_detach_computation(&mut self, id: u64) {
        let Some(mut fut) = self.pending_detach_computations.remove(&id) else {
            return;
        };
        let (timeout, at_stage) = {
            let pending = self.detach_inflight.get(&id).expect("inflight present when forcing");
            (pending.dist.force_timeout().expect("force only for sampled DurationDist"), pending.at_stage.clone())
        };
        let result = match std::future::Future::poll(fut.as_mut(), &mut Context::from_waker(Waker::noop())) {
            Poll::Ready(result) => result,
            Poll::Pending => match self.tokio_handle.block_on(async { tokio::time::timeout(timeout, fut).await }) {
                Ok(result) => result,
                Err(_elapsed) => {
                    panic!("detach effect on `{at_stage}` exceeded force timeout {timeout:?}")
                }
            },
        };
        self.provide_detach_result(id, result);
    }

    fn try_deliver_detach(&mut self, id: u64) {
        let Some(pending) = self.detach_inflight.get(&id) else {
            return;
        };
        if !pending.time_ready {
            return;
        }
        if pending.result.is_none() && pending.force_on_ready {
            self.force_detach_computation(id);
            return;
        }
        if pending.result.is_none() {
            return;
        }
        let pending = self.detach_inflight.remove(&id).expect("just checked");
        self.pending_detach_computations.remove(&id);
        self.deliver_detach_message(pending.at_stage, pending.result.expect("just checked"));
    }

    fn deliver_detach_message(&mut self, at_stage: Name, msg: Box<dyn SendData>) {
        match deliver_message(&mut self.stages, self.mailbox_size, at_stage.clone(), msg) {
            DeliverMessageResult::Delivered(_) => {}
            DeliverMessageResult::Full(_, msg) => {
                self.undelivered_detaches.push_back((at_stage, msg));
            }
            DeliverMessageResult::NotFound => {
                tracing::debug!(name = %at_stage, "detach result dropped: stage gone");
            }
        }
    }

    fn flush_undelivered_detaches(&mut self) {
        let mut parked = VecDeque::new();
        while let Some((name, msg)) = self.undelivered_detaches.pop_front() {
            match deliver_message(&mut self.stages, self.mailbox_size, name.clone(), msg) {
                DeliverMessageResult::Delivered(_) => {}
                DeliverMessageResult::Full(_, msg) => {
                    parked.push_back((name, msg));
                    parked.append(&mut self.undelivered_detaches);
                    break;
                }
                DeliverMessageResult::NotFound => {
                    tracing::debug!(name = %name, "detach result dropped: stage gone");
                }
            }
        }
        self.undelivered_detaches = parked;
    }

    fn apply_external_overrides(
        &mut self,
        mut effect: Box<dyn ExternalEffect>,
    ) -> Result<Box<dyn ExternalEffect>, Box<dyn SendData>> {
        let mut idx = 0;
        while idx < self.overrides.len() {
            use override_external_effect::OverrideResult::*;
            let over = &mut self.overrides[idx];
            match over.transform(effect) {
                NoMatch(effect2) => {
                    effect = effect2;
                    idx += 1;
                }
                Handled(msg) => {
                    if over.register_use_and_get_removal() {
                        self.overrides.remove(idx);
                    }
                    return Err(msg);
                }
                Replaced(effect2) => {
                    effect = effect2;
                    if over.register_use_and_get_removal() {
                        self.overrides.remove(idx);
                    } else {
                        idx += 1;
                    }
                }
            }
        }
        Ok(effect)
    }

    fn handle_detach(&mut self, at_stage: Name, effect: Box<dyn ExternalEffect>) -> Option<Blocked> {
        let dist = effect.simulated_duration_dist();
        let data = skip_if_terminated(self.stages.get_mut(&at_stage), &at_stage)?;
        let inject = resume_detach_internal(data, &mut |name, response| {
            tracing::debug!(%name, ?response, "enqueuing stage");
            self.runnable.push_back((name, response));
        })
        .expect("detach effect is always runnable");
        let id = self.begin_detach(at_stage.clone(), dist);
        if let Some(pending) = self.detach_inflight.get_mut(&id) {
            pending.inject = Some(inject);
        }
        match self.apply_external_overrides(effect) {
            Err(msg) => self.provide_detach_result(id, msg),
            Ok(effect) => {
                self.pending_detach_computations.insert(id, effect.run(self.resources.clone()));
                self.try_deliver_detach(id);
            }
        }
        None
    }

    fn try_deliver_external(&mut self, at_stage: &Name) {
        let Some(pending) = self.external_inflight.get(at_stage) else {
            return;
        };
        if !pending.time_ready {
            return;
        }
        if pending.result.is_none() && pending.force_on_ready {
            self.force_scheduled_computation(at_stage);
            return;
        }
        if pending.result.is_none() {
            return;
        }
        // World-provided UntilResolved results abandon the stored Future.
        self.pending_computations.remove(at_stage);
        let pending = self.external_inflight.remove(at_stage).expect("just checked");
        let Some(data) = self.stages.get_mut(at_stage) else {
            tracing::warn!(name = %at_stage, "stage was terminated, skipping external effect delivery");
            return;
        };
        resume_external_internal(data, pending.result.expect("just checked"), &mut |name, response| {
            tracing::debug!(%name, ?response, "enqueuing stage");
            self.runnable.push_back((name, response));
        })
        .expect("external effect is always runnable");
    }

    /// Place messages in the given stage’s mailbox, but don’t resume it.
    /// The next message will be consumed when resuming an [`Effect::Receive`]
    /// for this stage.
    ///
    /// # Panics
    ///
    /// Panics if the stage name does not exist (which may also happen due to termination).
    pub fn enqueue_msg<Msg: SendData>(&mut self, sr: impl AsRef<StageRef<Msg>>, msg: impl IntoIterator<Item = Msg>) {
        for msg in msg.into_iter() {
            let (name, leftover, payload) = sr.as_ref().materialize_send(msg);
            if leftover.is_some() {
                panic!("cannot enqueue to a call-reply StageRef");
            }
            let ok = deliver_message(&mut self.stages, self.mailbox_size, name, payload);
            if matches!(ok, DeliverMessageResult::Full(..)) {
                panic!("stage `{}` mailbox is full", sr.as_ref().name());
            }
        }
    }

    /// Retrieve the number of messages currently in the given stage’s mailbox.
    ///
    /// # Panics
    ///
    /// Panics if the stage name does not exist (which may also happen due to termination).
    pub fn mailbox_len<Msg>(&self, sr: impl AsRef<StageRef<Msg>>) -> usize {
        let name = sr.as_ref().name();
        expect_stage(self.stages.get(name), name, "which has no mailbox").mailbox.len()
    }

    /// Capacity of each stage mailbox (the limit [`Self::enqueue_msg`] will panic on).
    pub fn mailbox_size(&self) -> usize {
        self.mailbox_size
    }

    /// Obtain a reference to the current state of the given stage.
    /// This only works while the stage is suspended on an [`Effect::Receive`]
    /// because otherwise the state is captured by the opaque `Future` returned
    /// from the state transition function.
    ///
    /// Returns `None` if the stage is not suspended on [`Effect::Receive`], panics if the
    /// state type is incorrect.
    ///
    /// # Panics
    ///
    /// Panics if the stage name does not exist (which may also happen due to termination).
    pub fn get_state<Msg, St: SendData>(&self, sr: &StageStateRef<Msg, St>) -> Option<&St> {
        let data = expect_stage(self.stages.get(sr.name()), sr.name(), "which has no state");
        match &data.state {
            StageState::Idle(state) => Some(state.cast_ref::<St>().expect("internal state type error")),
            _ => None,
        }
    }

    /// Assert that a simulation step can be taken, take it and return the resulting effect.
    pub fn effect(&mut self) -> Effect {
        self.try_effect().unwrap()
    }

    /// If any stage is runnable, run it and return the resulting effect; otherwise return
    /// the classification of why no step can be taken (can be because the network is idle
    /// and needs more inputs, it could be deadlocked, or a stage is still suspended on an
    /// effect other than send (the latter case is called “busy” for want of a better term).
    pub fn try_effect(&mut self) -> Result<Effect, Blocked> {
        if self.runnable.is_empty() {
            let reason = block_reason(self);
            tracing::debug!("blocking for reason: {:?}", reason);
            return Err(reason);
        }
        let (name, response) = self.eval_strategy.pick_runnable(&mut self.runnable);

        tracing::debug!(%name, "resuming stage");
        self.trace_buffer.lock().push_resume(&name, &response);

        let data = expect_stage(self.stages.get_mut(&name), &name, "which is not runnable");

        let effect = poll_stage(
            &self.trace_buffer,
            &self.schedule_ids,
            data,
            name,
            response,
            &self.effect,
            self.clock.now(self.global_epoch_offset),
        );

        if !matches!(effect, Effect::Receive { .. }) {
            self.trace_buffer.lock().push_suspend(&effect);
        }

        if let Effect::External { at_stage, effect } = &effect {
            let at_stage = at_stage.clone();
            let dist = effect.simulated_duration_dist();
            self.begin_external(at_stage, dist);
        }

        Ok(effect)
    }

    /// Try to deliver external messages to stages that are waiting for them.
    ///
    /// Returns `InputsResult::Delivered(names)` if any messages were delivered,
    /// or `InputsResult::Blocked(name)` if delivery is blocked because the given
    /// stage's mailbox is full.
    pub fn try_inputs(&mut self) -> InputsResult {
        let mut delivered = Vec::new();
        while let Some(mut envelope) = self.inputs.try_next() {
            let msg = replace(&mut envelope.msg, Box::new(()));
            match deliver_message(&mut self.stages, self.mailbox_size, envelope.name.clone(), msg) {
                DeliverMessageResult::Delivered(_) => {
                    delivered.push(envelope.name);
                    envelope.tx.send(()).ok();
                }
                DeliverMessageResult::NotFound => {
                    tracing::warn!(name = %envelope.name, msg = ?envelope.msg, "stage was terminated, skipping input delivery");
                    envelope.tx.send(()).ok();
                    continue; // stage was terminated
                }
                DeliverMessageResult::Full(_, msg) => {
                    envelope.msg = msg;
                    let name = envelope.name.clone();
                    self.inputs.put_back(envelope);
                    if delivered.is_empty() {
                        return InputsResult::Blocked(name);
                    } else {
                        break;
                    }
                }
            }
        }
        InputsResult::Delivered(delivered)
    }

    /// When external effects are currently unresolved, await either the resolution of an effect
    /// or the arrival of a new external input message.
    pub async fn await_external_effect(&mut self) -> Option<Name> {
        if self.pending_computations.is_empty() && self.pending_detach_computations.is_empty() {
            return None;
        }
        if let Some(name) = self.take_ready_computation(&mut Context::from_waker(Waker::noop())) {
            return Some(name);
        }
        if self.pending_computations.is_empty() && self.pending_detach_computations.is_empty() {
            return None;
        }

        let pending = &mut self.pending_computations;
        let pending_detach = &mut self.pending_detach_computations;
        let inputs = &mut self.inputs;
        let ready = select! {
            env = inputs.next() => {
                inputs.put_back(env);
                None
            }
            ready = std::future::poll_fn(|cx| {
                if let Some(ready) = Self::poll_ready_computations(pending, cx) {
                    return Poll::Ready(Some(ReadyComputation::Blocking(ready)));
                }
                if let Some(ready) = Self::poll_ready_computations(pending_detach, cx) {
                    return Poll::Ready(Some(ReadyComputation::Detach(ready)));
                }
                if pending.is_empty() && pending_detach.is_empty() {
                    Poll::Ready(None)
                } else {
                    Poll::Pending
                }
            }) => ready,
        };
        Some(self.finish_ready_computation(ready?))
    }

    fn take_ready_computation(&mut self, cx: &mut Context<'_>) -> Option<Name> {
        if let Some(ready) = Self::poll_ready_computations(&mut self.pending_computations, cx) {
            return Some(self.finish_ready_computation(ReadyComputation::Blocking(ready)));
        }
        if let Some(ready) = Self::poll_ready_computations(&mut self.pending_detach_computations, cx) {
            return Some(self.finish_ready_computation(ReadyComputation::Detach(ready)));
        }
        None
    }

    fn finish_ready_computation(&mut self, ready: ReadyComputation) -> Name {
        match ready {
            ReadyComputation::Blocking((name, result)) => {
                self.provide_external_result(name.clone(), result);
                name
            }
            ReadyComputation::Detach((id, result)) => {
                let name =
                    self.detach_inflight.get(&id).map(|p| p.at_stage.clone()).unwrap_or_else(|| BLACKHOLE_NAME.clone());
                self.provide_detach_result(id, result);
                name
            }
        }
    }

    /// Wait for a message to be enqueued via an external input to the simulation.
    pub async fn await_external_input(&mut self) {
        let envelope = self.inputs.next().await;
        tracing::debug!(target = %envelope.name, "awaited external input received");
        self.inputs.put_back(envelope);
    }

    /// Keep alternating between [`Self::run_until_blocked`] and
    /// [`Self::await_external_effect`] until the simulation is blocked
    /// without waiting for external effects to be resolved.
    pub fn run_until_blocked_incl_effects(&mut self) -> Blocked {
        loop {
            match self.run_until_sleeping_or_blocked() {
                Blocked::Busy { external_effects, .. } if external_effects > 0 => {
                    self.tokio_handle.clone().block_on(self.await_external_effect());
                }
                Blocked::Sleeping { .. } => {
                    assert!(self.skip_to_next_wakeup(None));
                }
                blocked => return blocked,
            }
        }
    }

    /// Keep on performing steps using [`Self::try_effect`] while possible and automatically
    /// resume send and receive effects based on availability of space or messages in the
    /// mailbox in question.
    ///
    /// See [`Self::run_until_sleeping_or_blocked`] for a variant that stops when the simulation is
    /// waiting for a wakeup.
    ///
    /// When hitting a [`breakpoint`](Self::breakpoint), the simulation will return
    /// `Blocked::Breakpoint`, which allows you to extract the effect in progress
    /// using [`Blocked::assert_breakpoint`]. The result can later be passed to
    /// [`Self::handle_effect`] to resume the stage in question.
    ///
    /// **NOTE** that `Receive` effects are implicitly attempted to be resumed after completing
    /// a `Send` operation to that stage or whenever starting `run_until_*` and the stage's mailbox
    /// is not empty.
    pub fn run_until_blocked(&mut self) -> Blocked {
        loop {
            match self.run_until_sleeping_or_blocked() {
                Blocked::Sleeping { .. } => assert!(self.skip_to_next_wakeup(None)),
                blocked => return blocked,
            }
        }
    }

    pub fn run_until_blocked_or_time(&mut self, time: Instant) -> Blocked {
        loop {
            match self.run_until_sleeping_or_blocked() {
                Blocked::Sleeping { next_wakeup } => {
                    if !self.skip_to_next_wakeup(Some(time)) {
                        return Blocked::Sleeping { next_wakeup };
                    }
                }
                blocked => return blocked,
            }
        }
    }

    pub fn run_until_blocked_or_time_incl_effects(&mut self, time: Instant) -> Blocked {
        loop {
            match self.run_until_sleeping_or_blocked() {
                Blocked::Busy { external_effects, .. } if external_effects > 0 => {
                    self.tokio_handle.clone().block_on(self.await_external_effect());
                }
                Blocked::Sleeping { next_wakeup } => {
                    if !self.skip_to_next_wakeup(Some(time)) {
                        return Blocked::Sleeping { next_wakeup };
                    }
                }
                blocked => return blocked,
            }
        }
    }

    /// Keep on performing steps using [`Self::try_effect`] while possible and automatically
    /// resume send and receive effects based on availability of space or messages in the
    /// mailbox in question. It stops when the simulation is waiting for a wakeup.
    ///
    /// See [`Self::run_until_blocked`] for a variant that automatically advances
    /// the clock.
    pub fn run_until_sleeping_or_blocked(&mut self) -> Blocked {
        self.receive_inputs();
        loop {
            if let Some(value) = self.run_effect() {
                return value;
            }
        }
    }

    // TODO: shouldn’t this have a clock ceiling?
    pub fn run_one_step(&mut self) -> Option<Blocked> {
        self.receive_inputs();
        match self.run_effect() {
            Some(Blocked::Busy { .. }) => {
                self.tokio_handle.clone().block_on(self.await_external_effect());
                None
            }
            Some(Blocked::Sleeping { .. }) => {
                assert!(self.skip_to_next_wakeup(None));
                None
            }
            other => other,
        }
    }

    pub fn receive_inputs(&mut self) {
        self.try_inputs();
        self.flush_undelivered_detaches();
        let receiving = self
            .stages
            .iter()
            .filter_map(|(n, d)| {
                matches!(d, StageData { waiting: Some(StageEffect::Receive), .. }).then_some(n.clone())
            })
            .collect::<Vec<_>>();
        for name in receiving {
            // ignore all errors since this is a purely optimistic wake-up
            resume_receive_internal(self, &name).ok();
        }
        self.flush_undelivered_detaches();
    }

    fn run_effect(&mut self) -> Option<Blocked> {
        let effect = match self.try_effect() {
            Ok(effect) => effect,
            Err(blocked) => return Some(blocked),
        };

        tracing::debug!(runnable = ?self.runnable.iter().map(|r| r.0.as_str()).collect::<Vec<&str>>(), ?effect, "run effect");

        for (name, predicate) in &self.breakpoints {
            if (predicate)(&effect) {
                tracing::debug!("breakpoint `{}` hit: {:?}", name, effect);
                return Some(Blocked::Breakpoint(name.clone(), effect));
            }
        }

        self.handle_effect(effect)
    }

    /// Handle the given effect as it would be by [`Self::run_until_sleeping_or_blocked`].
    /// This will resume the affected stage(s), it may involve multiple resumptions.
    ///
    /// Inputs to this method can be obtained from [`Self::effect`], [`Self::try_effect`]
    /// or [`Blocked::assert_breakpoint`].
    pub fn handle_effect(&mut self, effect: Effect) -> Option<Blocked> {
        let runnable = &mut self.runnable;
        let run = &mut |name, response| {
            tracing::debug!(%name, ?response, "enqueuing stage");
            runnable.push_back((name, response));
        };

        match effect {
            Effect::Receive { at_stage: to } => {
                match resume_receive_internal(self, &to) {
                    Ok(true) => {}
                    Ok(false) => {
                        // nothing in the mailbox
                        return None;
                    }
                    Err(err) => {
                        tracing::warn!(%to, ?err, "cannot resume receive, shutting down simulation");
                        let terminated =
                            err.downcast::<resume::UnsupervisedChildTermination>().map(|e| e.0).unwrap_or(to);
                        return Some(Blocked::Terminated(terminated));
                    }
                }
                let data_to = self.stages.get_mut(&to)?;
                // resuming receive has removed one message from the mailbox, so check for blocked senders
                let (from, msg) = data_to.senders.pop_front()?;
                post_message(data_to, self.mailbox_size, msg);
                let data_from = skip_if_terminated(self.stages.get_mut(&from), &from)?;
                resume_send_internal(
                    data_from,
                    &mut |name, response| {
                        tracing::debug!(%name, ?response, "enqueuing stage");
                        self.runnable.push_back((name, response));
                    },
                    to.clone(),
                    &mut None,
                )
                .expect("call is always runnable");
            }
            Effect::Send { from, to, .. } if to.is_empty() => {
                tracing::info!(stage = %from, "message send to blackhole dropped");
                let data_from = skip_if_terminated(self.stages.get_mut(&from), &from)?;
                resume_send_internal(data_from, run, to.clone(), &mut None).expect("call is always runnable");
            }
            Effect::Send { from, to, msg } => {
                let is_call = self
                    .stages
                    .get(&from)
                    .map(|d| matches!(d, StageData { waiting: Some(StageEffect::Send(_, Some(_), _)), .. }))
                    .unwrap_or_default();
                if is_call {
                    // sending stage is always resumed
                    // if the stage was killed while waiting for its turn in sending this response
                    // then the response is simply dropped and the call may time out
                    let data_from = skip_if_terminated(self.stages.get_mut(&from), &from)?;
                    let mut msg = Some(msg);
                    let id =
                        resume_send_internal(data_from, run, to.clone(), &mut msg).expect("call is always runnable");
                    if let Some(id) = id {
                        self.scheduled.remove(&id);
                        let data_to = skip_if_terminated(self.stages.get_mut(&to), &to)?;
                        // call response races with other responses and timeout, so failure to resume is okay
                        resume_call_internal(
                            data_to,
                            run,
                            Some(id),
                            msg.expect("scheduled call response must preserve payload"),
                        )
                        .ok();
                    }
                } else {
                    let mb = self.mailbox_size;
                    let resume = match deliver_message(&mut self.stages, mb, to.clone(), msg) {
                        DeliverMessageResult::Delivered(data_to) => {
                            // `to` may not be suspended on receive, so failure to resume is okay
                            let name = data_to.name.clone();
                            if let Err(err) = resume_receive_internal(self, &name) {
                                tracing::warn!(%from, %to, ?err, "cannot deliver send, shutting down simulation");
                                let terminated =
                                    err.downcast::<resume::UnsupervisedChildTermination>().map(|e| e.0).unwrap_or(name);
                                return Some(Blocked::Terminated(terminated));
                            }
                            Some(from)
                        }
                        DeliverMessageResult::Full(data_to, send_data) => {
                            data_to.senders.push_back((from, send_data));
                            None
                        }
                        DeliverMessageResult::NotFound => {
                            tracing::debug!(stage = %to, "message send to terminated stage dropped");
                            Some(from)
                        }
                    };
                    if let Some(from) = resume {
                        let data_from = skip_if_terminated(self.stages.get_mut(&from), &from)?;
                        resume_send_internal(
                            data_from,
                            &mut |name, response| {
                                tracing::debug!(%name, ?response, "enqueuing stage");
                                self.runnable.push_back((name, response));
                            },
                            to.clone(),
                            &mut None,
                        )
                        .expect("call is always runnable");
                    }
                }
            }
            Effect::Call { from, to, duration: _, msg } => {
                if let Err(err) = resume_call_send_internal(self, from.clone(), to.clone(), msg) {
                    tracing::warn!(%from, %to, %err, "couldn’t deliver call effect");
                    return Some(Blocked::Terminated(from));
                }
            }
            Effect::Clock { at_stage } => {
                let data = skip_if_terminated(self.stages.get_mut(&at_stage), &at_stage)?;
                let now = self.clock.now(self.global_epoch_offset);
                resume_clock_internal(data, run, now).expect("clock effect is always runnable");
            }
            Effect::Wait { at_stage, duration } => {
                let now = self.clock.now(self.global_epoch_offset);
                let id = self.schedule_ids.next_at(now + duration);
                self.schedule_wakeup(id, move |sim| {
                    let Some(data) = sim.stages.get_mut(&at_stage) else {
                        tracing::warn!(name = %at_stage, "stage was terminated, skipping wait effect delivery");
                        return;
                    };
                    resume_wait_internal(
                        data,
                        &mut |name, response| {
                            tracing::debug!(%name, ?response, "enqueuing stage");
                            sim.runnable.push_back((name, response));
                        },
                        sim.clock.now(sim.global_epoch_offset),
                    )
                    .expect("wait effect is always runnable");
                });
            }
            Effect::Schedule { at_stage, msg, id } => {
                let data = skip_if_terminated(self.stages.get_mut(&at_stage), &at_stage)?;
                let limit = self.priority_mailbox_size;
                if data.scheduled_pending >= limit {
                    panic!(
                        "stage `{}` exceeded priority mailbox size ({limit}): too many outstanding scheduled messages",
                        data.name
                    );
                }
                data.scheduled_pending += 1;
                resume_schedule_internal(data, run, id).expect("schedule effect is always runnable");
                let now = self.clock.now(self.global_epoch_offset);
                if id.time() > now {
                    self.schedule_wakeup(id, {
                        move |sim| {
                            deliver_priority(sim, at_stage, msg);
                        }
                    });
                } else {
                    deliver_priority(self, at_stage, msg);
                }
            }
            Effect::CancelSchedule { at_stage, id } => {
                let cancelled = self.scheduled.remove(&id).is_some();
                let data = skip_if_terminated(self.stages.get_mut(&at_stage), &at_stage)?;
                if cancelled {
                    data.scheduled_pending = data.scheduled_pending.saturating_sub(1);
                }
                resume_cancel_schedule_internal(data, run, cancelled)
                    .expect("cancel_schedule effect is always runnable");
            }
            Effect::External { at_stage, effect } => match self.apply_external_overrides(effect) {
                Err(msg) => {
                    self.provide_external_result(at_stage, msg);
                    return None;
                }
                Ok(effect) => {
                    let name = at_stage.clone();
                    self.pending_computations.insert(name.clone(), effect.run(self.resources.clone()));
                    self.try_deliver_external(&name);
                }
            },
            Effect::Detach { at_stage, effect } => return self.handle_detach(at_stage, effect),
            Effect::Terminate { at_stage } => {
                tracing::info!(stage = %at_stage, "terminated");
                let (supervised_by, msg) = self.terminate_stage(at_stage.clone(), TerminationReason::Voluntary)?;
                if supervised_by == *BLACKHOLE_NAME {
                    // top-level stage terminated, terminate the simulation
                    self.terminate.send_replace(true);
                    return Some(Blocked::Terminated(at_stage));
                }
                let supervisor =
                    expect_stage(self.stages.get_mut(&supervised_by), &supervised_by, "which cannot supervise");
                supervisor.tombstones.push_back(msg);
                if let Err(err) = resume_receive_internal(self, &supervised_by) {
                    tracing::warn!(%supervised_by, ?err, "shutting down simulation");
                    let terminated =
                        err.downcast::<resume::UnsupervisedChildTermination>().map(|e| e.0).unwrap_or(supervised_by);
                    return Some(Blocked::Terminated(terminated));
                }
            }
            Effect::AddStage { at_stage, name } => {
                let name = stage_name(&mut self.stage_count, name.as_str());
                let data = skip_if_terminated(self.stages.get_mut(&at_stage), &at_stage)?;
                resume_add_stage_internal(data, run, name).expect("add stage effect is always runnable");
            }
            Effect::WireStage { at_stage, name, initial_state, tombstone } => {
                self.trace_buffer.lock().push_state(&name, &initial_state);
                let data = skip_if_terminated(self.stages.get_mut(&at_stage), &at_stage)?;
                let transition = resume_wire_stage_internal(data, run).expect("wire stage effect is always runnable");
                let tombstone = tombstone.try_cast::<CanSupervise>().err();

                if self.virtual_child_stages {
                    // Virtual mode: parent has been successfully resumed (via resume_wire_stage_internal),
                    // the effect and the child's intended initial state are recorded in the trace,
                    // but we deliberately do not materialize a runnable child stage.
                    // Sends to the returned StageRef will later be NotFound (dropped).
                    tracing::debug!(parent = %at_stage, child = %name, "wire_up completed in virtual-child mode (no stage inserted)");
                } else {
                    self.stages.insert(
                        name.clone(),
                        StageData {
                            name,
                            mailbox: VecDeque::new(),
                            priority: VecDeque::new(),
                            tombstones: VecDeque::new(),
                            state: StageState::Idle(initial_state),
                            transition: (transition)(self.effect.clone()),
                            waiting: Some(StageEffect::Receive),
                            senders: VecDeque::new(),
                            scheduled_pending: 0,
                            supervised_by: at_stage,
                            tombstone,
                        },
                    );
                }
            }
        }
        None
    }

    /// Recursively terminate the given stage and all its children.
    ///
    /// This also cleans up the state of all terminated stages in the simulation,
    /// like run queue and sleeping message senders.
    fn terminate_stage(
        &mut self,
        at_stage: Name,
        reason: TerminationReason,
    ) -> Option<(Name, Result<Box<dyn SendData>, Name>)> {
        // TODO(network):
        // - add kill switch to scheduled external effects to terminate them
        // - record source stage for scheduled messages to remove them

        let Some(data) = self.stages.get_mut(&at_stage) else {
            tracing::warn!(name = %at_stage, "stage was already terminated, skipping terminate stage effect");
            return None;
        };

        // parent state is dropped before the children, but amaru-pure-stage states are just dumb data
        // anyway, so this should usually be what we want
        data.state = StageState::Terminating;

        // clean up simulation state for this stage
        self.runnable.retain(|(n, _)| n != &at_stage);
        self.external_inflight.remove(&at_stage);
        self.pending_computations.remove(&at_stage);
        let drop_ids: Vec<u64> = self
            .detach_inflight
            .iter()
            .filter_map(|(id, pending)| (pending.at_stage == at_stage).then_some(*id))
            .collect();
        for id in drop_ids {
            self.detach_inflight.remove(&id);
            self.pending_detach_computations.remove(&id);
        }
        self.undelivered_detaches.retain(|(n, _)| n != &at_stage);

        let runnable = &mut self.runnable;
        let run = &mut |name, response| {
            tracing::debug!(%name, ?response, "enqueuing stage");
            runnable.push_back((name, response));
        };
        let senders = std::mem::take(&mut data.senders);
        for (waiting, _) in senders {
            let data = expect_stage(self.stages.get_mut(&waiting), &waiting, "which cannot send");
            if let Err(err) = resume_send_internal(data, run, at_stage.clone(), &mut None) {
                tracing::error!(from = %waiting, to = %at_stage, %err, "failed to resume send");
                continue;
            };
        }

        let children = self
            .stages
            .iter()
            .filter(|(_, d)| matches!(d, StageData { supervised_by, .. } if supervised_by == &at_stage))
            .map(|(n, _)| n.clone())
            .collect::<Vec<_>>();
        for child in children {
            tracing::info!(stage = %child, parent = %at_stage, "terminating child stage");
            self.terminate_stage(child, TerminationReason::Aborted);
        }
        self.trace_buffer.lock().push_terminated(&at_stage, reason);
        let Some(stage) = self.stages.remove(&at_stage) else {
            unreachable!();
        };
        Some((stage.supervised_by, stage.tombstone.ok_or(at_stage)))
    }

    /// If a stage is Idle, it is waiting for Receive and NOT runnable.
    /// If a stage is Running, it may be waiting for a non-Receive effect and may be runnable.
    /// If a stage is Failed, it is not waiting for any effect and is not runnable.
    /// A non-Failed stage is either waiting or runnable.
    #[cfg(test)]
    fn invariants(&self) {
        for (name, data) in &self.stages {
            let waiting = &data.waiting;
            match &data.state {
                StageState::Idle(_) => {
                    if !matches!(waiting, Some(StageEffect::Receive)) {
                        panic!("stage `{name}` is Idle but waiting for {waiting:?}");
                    }
                }
                StageState::Running(_) => {
                    if matches!(waiting, Some(StageEffect::Receive)) {
                        panic!("stage `{name}` is Running but waiting for Receive");
                    }
                }
                StageState::Terminating => {
                    if waiting.is_some() {
                        panic!("stage `{name}` is Terminating but waiting for {waiting:?}");
                    }
                    return;
                }
            }
            let waiting = waiting.is_some();
            let runnable = self.runnable.iter().any(|(n, _)| n == name);
            if waiting && runnable {
                panic!("stage `{name}` is waiting for an effect and runnable");
            }
            if !waiting && !runnable {
                panic!("stage `{name}` is not waiting for an effect and not runnable");
            }
        }
    }

    /// Resume an [`Effect::Receive`].
    pub fn resume_receive<Msg>(&mut self, at_stage: impl AsRef<StageRef<Msg>>) -> anyhow::Result<()> {
        resume_receive_internal(self, at_stage.as_ref().name()).and_then(|resumed| {
            if resumed { Ok(()) } else { Err(anyhow::anyhow!("stage was not waiting for a receive effect")) }
        })
    }

    /// Resume an [`Effect::Send`].
    ///
    /// # Panics
    ///
    /// Panics if the stage name does not exist (which may also happen due to termination).
    pub fn resume_send<Msg1, Msg2: SendData>(
        &mut self,
        from: impl AsRef<StageRef<Msg1>>,
        to: impl AsRef<StageRef<Msg2>>,
        mut msg: Option<Msg2>,
    ) -> anyhow::Result<()> {
        let to = to.as_ref();
        if to.extra().is_none()
            && let Some(msg) = msg.take()
            && deliver_message(&mut self.stages, self.mailbox_size, to.name().clone(), Box::new(msg)).is_full()
        {
            anyhow::bail!("mailbox is full while resuming send");
        }

        let from_name = from.as_ref().name();
        let data = expect_stage(self.stages.get_mut(from_name), from_name, "which cannot send");
        let mut msg = msg.map(|msg| Box::new(msg) as Box<dyn SendData>);
        let id = resume_send_internal(
            data,
            &mut |name, response| {
                tracing::debug!(%name, ?response, "enqueuing stage");
                self.runnable.push_back((name, response));
            },
            to.name().clone(),
            &mut msg,
        )?;

        if let Some(id) = id
            && let Some(msg) = msg
        {
            self.scheduled.remove(&id);
            let data = expect_stage(self.stages.get_mut(to.name()), to.name(), "which cannot call");
            resume_call_internal(
                data,
                &mut |name, response| {
                    tracing::debug!(%name, ?response, "enqueuing stage");
                    self.runnable.push_back((name, response));
                },
                Some(id),
                msg,
            )?;
        }

        Ok(())
    }

    /// Resume an [`Effect::Clock`].
    ///
    /// # Panics
    ///
    /// Panics if the stage name does not exist (which may also happen due to termination).
    pub fn resume_clock<Msg>(&mut self, at_stage: impl AsRef<StageRef<Msg>>, time: Instant) -> anyhow::Result<()> {
        let name = at_stage.as_ref().name();
        let data = expect_stage(self.stages.get_mut(name), name, "which cannot ask for the clock");
        let time = Instant { inner: time.inner, global_epoch_offset: self.global_epoch_offset };
        resume_clock_internal(
            data,
            &mut |name, response| {
                tracing::debug!(%name, ?response, "enqueuing stage");
                self.runnable.push_back((name, response));
            },
            time,
        )
    }

    /// Resume an [`Effect::Wait`].
    ///
    /// The given time is the clock when the stage wakes up.
    ///
    /// # Panics
    ///
    /// Panics if the stage name does not exist (which may also happen due to termination).
    pub fn resume_wait<Msg>(&mut self, at_stage: impl AsRef<StageRef<Msg>>, time: Instant) -> anyhow::Result<()> {
        let name = at_stage.as_ref().name();
        let data = expect_stage(self.stages.get_mut(name), name, "which cannot wait");
        let time = Instant { inner: time.inner, global_epoch_offset: self.global_epoch_offset };
        resume_wait_internal(
            data,
            &mut |name, response| {
                tracing::debug!(%name, ?response, "enqueuing stage");
                self.runnable.push_back((name, response));
            },
            time,
        )
    }

    /// Resume the sending part of a [`Effect::Call`].
    ///
    /// # Panics
    ///
    /// Panics if the stage name does not exist (which may also happen due to termination).
    pub fn resume_call_send<Msg: SendData, Msg2: SendData>(
        &mut self,
        from: impl AsRef<StageRef<Msg>>,
        to: impl AsRef<StageRef<Msg2>>,
        msg: Msg2,
    ) -> anyhow::Result<()> {
        resume_call_send_internal(self, from.as_ref().name().clone(), to.as_ref().name().clone(), Box::new(msg))
            .and_then(
                |resumed| {
                    if resumed { Ok(()) } else { Err(anyhow::anyhow!("stage was not waiting for a call effect")) }
                },
            )
    }

    /// Resume an [`Effect::Call`].
    ///
    /// # Panics
    ///
    /// Panics if the stage name does not exist (which may also happen due to termination).
    pub fn resume_call<Msg: SendData, Resp: SendData>(
        &mut self,
        at_stage: impl AsRef<StageRef<Msg>>,
        msg: Resp,
    ) -> anyhow::Result<()> {
        let at_stage = at_stage.as_ref();
        let data = expect_stage(self.stages.get_mut(at_stage.name()), at_stage.name(), "which cannot make a call");
        resume_call_internal(
            data,
            &mut |name, response| {
                tracing::debug!(%name, ?response, "enqueuing stage");
                self.runnable.push_back((name, response));
            },
            None,
            Box::new(msg),
        )
    }

    /// Resume an [`Effect::External`].
    ///
    /// Returns an error if the stage does not exist (including after termination) or is not
    /// waiting for an external effect.
    pub fn resume_external_box(&mut self, at_stage: impl AsRef<Name>, result: Box<dyn SendData>) -> anyhow::Result<()> {
        let at_stage = at_stage.as_ref().clone();
        {
            let data = self.stages.get_mut(&at_stage).ok_or_else(|| {
                anyhow::anyhow!("stage `{at_stage}` not found, which cannot receive external effects")
            })?;
            if !matches!(data.waiting, Some(StageEffect::External(_))) {
                anyhow::bail!("stage `{at_stage}` was not waiting for an external effect, but {:?}", data.waiting);
            }
        }
        self.provide_external_result(at_stage, result);
        Ok(())
    }

    /// Resume an [`Effect::External`].
    ///
    /// Returns an error if the stage does not exist (including after termination) or is not
    /// waiting for an external effect.
    pub fn resume_external<Eff: ExternalEffectAPI>(
        &mut self,
        at_stage: impl AsRef<Name>,
        result: Eff::Response,
    ) -> anyhow::Result<()> {
        let at_stage = at_stage.as_ref().clone();
        {
            let data = self.stages.get_mut(&at_stage).ok_or_else(|| {
                anyhow::anyhow!("stage `{at_stage}` not found, which cannot receive external effects")
            })?;
            if !matches!(data.waiting, Some(StageEffect::External(_))) {
                anyhow::bail!("stage `{at_stage}` was not waiting for an external effect, but {:?}", data.waiting);
            }
        }
        self.provide_external_result(at_stage, Box::new(result));
        Ok(())
    }

    /// Resume an [`Effect::AddStage`].
    ///
    /// # Panics
    ///
    /// Panics if the stage name does not exist (which may also happen due to termination).
    pub fn resume_add_stage<Msg>(&mut self, at_stage: impl AsRef<StageRef<Msg>>, name: Name) -> anyhow::Result<()> {
        let at_name = at_stage.as_ref().name();
        let data = expect_stage(self.stages.get_mut(at_name), at_name, "which cannot add a stage");
        resume_add_stage_internal(
            data,
            &mut |name, response| {
                tracing::debug!(%name, ?response, "enqueuing stage");
                self.runnable.push_back((name, response));
            },
            name,
        )
    }

    /// Resume an [`Effect::WireStage`].
    ///
    /// # Panics
    ///
    /// Panics if the stage name does not exist (which may also happen due to termination).
    pub fn resume_wire_stage<Msg>(
        &mut self,
        at_stage: impl AsRef<StageRef<Msg>>,
        name: Name,
        initial_state: Box<dyn SendData>,
        tombstone: Option<Box<dyn SendData>>,
    ) -> anyhow::Result<()> {
        let at_stage = at_stage.as_ref();
        let data = expect_stage(self.stages.get_mut(at_stage.name()), at_stage.name(), "which cannot wire a stage");
        let transition = resume_wire_stage_internal(data, &mut |name, response| {
            tracing::debug!(%name, ?response, "enqueuing stage");
            self.runnable.push_back((name, response));
        })?;

        if self.virtual_child_stages {
            tracing::debug!(parent = %at_stage.name(), child = %name, "resume_wire_stage in virtual-child mode (no stage inserted)");
        } else {
            self.stages.insert(
                name.clone(),
                StageData {
                    name,
                    mailbox: VecDeque::new(),
                    priority: VecDeque::new(),
                    tombstones: VecDeque::new(),
                    state: StageState::Idle(initial_state),
                    transition: (transition)(self.effect.clone()),
                    waiting: Some(StageEffect::Receive),
                    senders: VecDeque::new(),
                    scheduled_pending: 0,
                    supervised_by: at_stage.name().clone(),
                    tombstone,
                },
            );
        }
        Ok(())
    }
}

pub(super) fn skip_if_terminated<'a>(stage: Option<&'a mut StageData>, name: &Name) -> Option<&'a mut StageData> {
    if stage.is_none() {
        tracing::warn!(%name, "stage was terminated, skipping effect handling");
    }
    stage
}

pub(super) fn expect_stage<T>(stage: Option<T>, name: &Name, why: &str) -> T {
    stage.unwrap_or_else(|| panic!("stage `{name}` not found, {why}"))
}

impl StageGraphRunning for SimulationRunning {
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

pub struct OverrideResult<Eff: ExternalEffectAPI>(
    override_external_effect::OverrideResult<Box<Eff>, Box<dyn ExternalEffect>>,
);

impl<Eff: ExternalEffectAPI> OverrideResult<Eff> {
    /// Don't modify the given effect, it will be passed to later overrides unchanged.
    pub fn no_match(eff: Box<Eff>) -> Self {
        Self(override_external_effect::OverrideResult::NoMatch(eff))
    }
    /// Replace running the effect with the given response value.
    pub fn handled(response: <Eff as ExternalEffectAPI>::Response) -> Self {
        Self(override_external_effect::OverrideResult::Handled(Box::new(response)))
    }
    /// Run the given effect instead (which must return the same response type).
    ///
    /// The replacement is subject to further overrides (i.e. those registered later).
    pub fn replaced<E>(eff: E) -> Self
    where
        E: ExternalEffectAPI<Response = Eff::Response>,
    {
        Self(override_external_effect::OverrideResult::Replaced(Box::new(eff)))
    }
}

// module to make fields actually private
mod override_external_effect {
    use super::*;

    pub struct OverrideExternalEffect {
        remaining: usize,
        transform: Box<
            dyn FnMut(Box<dyn ExternalEffect>) -> OverrideResult<Box<dyn ExternalEffect>, Box<dyn ExternalEffect>>
                + Send
                + 'static,
        >,
    }

    /// The result of an override.
    ///
    /// This is used to determine what to do with an effect that has been passed to an override.
    pub enum OverrideResult<In, Out> {
        /// The effect was not handled and shall be passed to overrides installed later than this one.
        NoMatch(In),
        /// The effect was handled and the message shall be delivered to the stage as the result.
        Handled(Box<dyn SendData>),
        /// The effect was replaced by this new effect that will be run instead.
        Replaced(Out),
    }

    impl OverrideExternalEffect {
        pub fn new(
            remaining: usize,
            transform: Box<
                dyn FnMut(Box<dyn ExternalEffect>) -> OverrideResult<Box<dyn ExternalEffect>, Box<dyn ExternalEffect>>
                    + Send
                    + 'static,
            >,
        ) -> Self {
            Self { remaining, transform }
        }

        pub fn transform(
            &mut self,
            effect: Box<dyn ExternalEffect>,
        ) -> OverrideResult<Box<dyn ExternalEffect>, Box<dyn ExternalEffect>> {
            (self.transform)(effect)
        }

        pub fn register_use_and_get_removal(&mut self) -> bool {
            if self.remaining == usize::MAX {
                return false;
            }
            self.remaining -= 1;
            self.remaining == 0
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum InputsResult {
    Delivered(Vec<Name>),
    Blocked(Name),
}

fn block_reason(sim: &SimulationRunning) -> Blocked {
    debug_assert!(sim.runnable.is_empty(), "runnable must be empty");
    if sim.stages.values().filter_map(|d| d.waiting.as_ref()).all(|v| matches!(v, StageEffect::Receive)) {
        let unresolved_detaches = sim.detach_inflight.len();
        if unresolved_detaches > 0 && sim.next_wakeup().is_none() {
            return Blocked::Busy { stages: Vec::new(), external_effects: unresolved_detaches };
        }
        if let Some(next_wakeup) = sim.next_wakeup() {
            return Blocked::Sleeping { next_wakeup };
        } else {
            return Blocked::Idle;
        }
    }
    let mut send = Vec::new();
    let mut busy = Vec::new();
    let mut sleep = Vec::new();
    for (k, v) in sim.stages.iter().filter_map(|(k, d)| d.waiting.as_ref().map(|w| (k, w))) {
        match v {
            StageEffect::Send(name, None, _msg) => {
                send.push(SendBlock { from: k.clone(), to: name.clone(), is_call: false })
            }
            StageEffect::Receive => {}
            StageEffect::Wait(..) => sleep.push(k.clone()),
            StageEffect::External(_) => match sim.external_inflight.get(k) {
                // Deadline not yet reached: sleep even if the Future is still running.
                Some(pending) if !pending.time_ready => sleep.push(k.clone()),
                _ => busy.push(k.clone()),
            },
            StageEffect::Call(_, _, CallExtra::Scheduled(id)) if sim.scheduled.contains(id) => sleep.push(k.clone()),
            _ => busy.push(k.clone()),
        }
    }

    if !busy.is_empty() {
        Blocked::Busy {
            stages: busy,
            external_effects: sim.pending_computations.len() + sim.pending_detach_computations.len(),
        }
    } else if !sleep.is_empty() {
        Blocked::Sleeping { next_wakeup: sim.next_wakeup().expect("stages are waiting for a wait effect") }
    } else if !send.is_empty() {
        Blocked::Deadlock(send)
    } else {
        Blocked::Idle
    }
}

/// Poll a stage and return the effect that should be run next.
///
/// It is used to poll a stage and return the effect that should be run next.
/// The `response` is the input with which the stage is resumed.
pub(crate) fn poll_stage(
    trace_buffer: &Arc<Mutex<TraceBuffer>>,
    schedule_ids: &ScheduleIds,
    data: &mut StageData,
    name: Name,
    response: StageResponse,
    effect: &EffectBox,
    now: Instant,
) -> Effect {
    let StageState::Running(pin) = &mut data.state else {
        panic!("runnable stage `{name}` is not running but {:?}", data.state);
    };

    *effect.lock() = Some(Right(response));
    let result = pin.as_mut().poll(&mut Context::from_waker(Waker::noop()));

    if let Poll::Ready(state) = result {
        trace_buffer.lock().push_state(&name, &state);
        data.state = StageState::Idle(state);
        data.waiting = Some(StageEffect::Receive);
        Effect::Receive { at_stage: name }
    } else {
        let stage_effect = match effect.lock().take() {
            Some(Left(effect)) => effect,
            Some(Right(response)) => {
                panic!("found response {response:?} instead of effect when polling stage `{name}`")
            }
            None => {
                panic!("stage `{name}` returned without awaiting any tracked effect")
            }
        };
        let (wait_effect, effect) = stage_effect.split(name.clone(), schedule_ids, now);
        if !matches!(wait_effect, StageEffect::Terminate) {
            data.waiting = Some(wait_effect);
        }
        effect
    }
}

enum DeliverMessageResult<'a> {
    Delivered(&'a mut StageData),
    Full(&'a mut StageData, Box<dyn SendData>),
    NotFound,
}

impl<'a> DeliverMessageResult<'a> {
    pub fn is_full(&self) -> bool {
        matches!(self, DeliverMessageResult::Full(..))
    }
}

/// Deliver a message to a stage.
///
/// Returns `true` if the message was delivered, `false` if the target mailbox
/// does not exist, or `Err` if the mailbox is full.
fn deliver_message(
    stages: &mut BTreeMap<Name, StageData>,
    mailbox_size: usize,
    name: Name,
    msg: Box<dyn SendData>,
) -> DeliverMessageResult<'_> {
    let Some(data) = stages.get_mut(&name) else {
        return DeliverMessageResult::NotFound;
    };

    post_message(data, mailbox_size, msg)
}

fn post_message(data: &mut StageData, mailbox_size: usize, msg: Box<dyn SendData>) -> DeliverMessageResult<'_> {
    if data.mailbox.len() >= mailbox_size {
        return DeliverMessageResult::Full(data, msg);
    }
    data.mailbox.push_back(msg);
    DeliverMessageResult::Delivered(data)
}

/// Deliver a due self-scheduled message into the stage's priority ingress.
///
/// Priority messages never compete with the bulk mailbox. The outstanding budget was
/// already reserved when the schedule effect ran (`scheduled_pending`).
fn deliver_priority(sim: &mut SimulationRunning, at_stage: Name, msg: Box<dyn SendData>) {
    let limit = sim.priority_mailbox_size;
    let Some(data) = sim.stages.get_mut(&at_stage) else {
        tracing::warn!(name = %at_stage, "stage was terminated, skipping scheduled message delivery");
        return;
    };
    if data.priority.len() >= limit {
        panic!("stage `{}` exceeded priority mailbox size ({limit}): too many due scheduled messages", data.name);
    }
    data.priority.push_back(msg);
    let name = data.name.clone();
    // Stage may already be waiting on Receive; wake it so the priority message is not stuck.
    let _ = resume_receive_internal(sim, &name);
}

#[test]
fn simulation_invariants() {
    use crate::StageGraph;

    tracing_subscriber::fmt()
        .with_test_writer()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init()
        .ok();

    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    struct Msg(Option<StageRef<()>>);

    let mut network = crate::simulation::SimulationBuilder::default();
    let stage = network.stage("stage", async |_state, _msg: Msg, eff| {
        eff.send(&eff.me(), Msg(None)).await;
        eff.clock().await;
        eff.wait(std::time::Duration::from_secs(1)).await;
        eff.call(&eff.me(), std::time::Duration::from_secs(1), |cr| Msg(Some(cr))).await;
        true
    });

    let stage = network.wire_up(stage, false);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut sim = network.run(rt.handle());

    #[expect(clippy::type_complexity)]
    let ops: [(
        Box<dyn Fn(&Effect) -> bool>,
        Box<dyn Fn(&mut SimulationRunning, &StageRef<Msg>) -> anyhow::Result<()>>,
        &'static str,
    ); _] = [
        (
            Box::new(|eff: &Effect| matches!(eff, Effect::Receive { .. })),
            Box::new(|sim, stage| sim.resume_receive(stage)),
            "resume_receive",
        ),
        (
            Box::new(|eff: &Effect| matches!(eff, Effect::Send { .. })),
            Box::new(|sim, stage| sim.resume_send(stage, stage, Some(Msg(None)))),
            "resume_send",
        ),
        (
            Box::new(|eff: &Effect| matches!(eff, Effect::Clock { .. })),
            Box::new(|sim, stage| sim.resume_clock(stage, Instant::now())),
            "resume_clock",
        ),
        (
            Box::new(|eff: &Effect| matches!(eff, Effect::Wait { .. })),
            Box::new(|sim, stage| sim.resume_wait(stage, Instant::now())),
            "resume_wait",
        ),
        (
            Box::new(|eff: &Effect| matches!(eff, Effect::Call { .. })),
            Box::new(|sim, stage| sim.resume_call(stage, ())),
            "resume_call",
        ),
    ];

    sim.invariants();
    sim.enqueue_msg(&stage, [Msg(None)]);
    sim.invariants();

    for idx in 0..ops.len() {
        let effect = if idx == 0 { Effect::Receive { at_stage: "stage".into() } } else { sim.effect() };
        tracing::info!(?effect, "effect");
        assert!(ops[idx].0(&effect), "effect {effect:?} should match predicate for `{idx}`");
        for (pred, op, name) in &ops {
            if !pred(&effect) {
                tracing::info!("op `{}` should not work", name);
                op(&mut sim, &stage.clone().without_state()).unwrap_err();
                sim.invariants();
            }
        }
        for (pred, op, name) in &ops {
            if pred(&effect) {
                tracing::info!("op `{}` should work", name);
                op(&mut sim, &stage.clone().without_state()).unwrap();
                sim.invariants();
            }
        }
    }
    tracing::info!("final invariants");
    sim.effect().assert_receive(&stage);
    let state = sim.get_state(&stage).unwrap();
    assert!(state);
}
