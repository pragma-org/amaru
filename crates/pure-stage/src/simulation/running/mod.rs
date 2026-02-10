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

#![expect(
    clippy::wildcard_enum_match_arm,
    clippy::unwrap_used,
    clippy::panic,
    clippy::expect_used
)]

use crate::{
    BLACKHOLE_NAME, BoxFuture, Effect, ExternalEffect, ExternalEffectAPI, Instant, Name, Resources,
    ScheduleId, SendData, StageRef, StageResponse,
    adapter::{Adapter, StageOrAdapter, find_recipient},
    effect::{CallExtra, CanSupervise, ScheduleIds, StageEffect},
    effect_box::EffectBox,
    simulation::{
        blocked::{Blocked, SendBlock},
        inputs::Inputs,
        random::EvalStrategy,
        running::{
            resume::{
                resume_add_stage_internal, resume_call_internal, resume_call_send_internal,
                resume_cancel_schedule_internal, resume_clock_internal, resume_contramap_internal,
                resume_external_internal, resume_receive_internal, resume_schedule_internal,
                resume_send_internal, resume_wait_internal, resume_wire_stage_internal,
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
use either::Either::{Left, Right};
use futures_util::{StreamExt, stream::FuturesUnordered};
use override_external_effect::OverrideExternalEffect;
pub use override_external_effect::OverrideResult;
use parking_lot::Mutex;
use std::{
    collections::{BTreeMap, VecDeque},
    mem::replace,
    sync::Arc,
    task::{Context, Poll, Waker},
};
use tokio::{runtime::Handle, select, sync::watch};

mod resume;
mod scheduled_runnables;

/// A handle to a running [`SimulationBuilder`](crate::effect_box::SimulationBuilder).
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
    stages: BTreeMap<Name, StageOrAdapter<StageData>>,
    stage_count: usize,
    inputs: Inputs,
    effect: EffectBox,
    clock: Arc<dyn Clock + Send + Sync>,
    resources: Resources,
    runnable: VecDeque<(Name, StageResponse)>,
    scheduled: ScheduledRunnables,
    mailbox_size: usize,
    overrides: Vec<OverrideExternalEffect>,
    breakpoints: Vec<(Name, Box<dyn Fn(&Effect) -> bool + Send + 'static>)>,
    schedule_ids: ScheduleIds,
    trace_buffer: Arc<Mutex<TraceBuffer>>,
    eval_strategy: Box<dyn EvalStrategy>,
    terminate: watch::Sender<bool>,
    termination: watch::Receiver<bool>,
    external_effects: FuturesUnordered<BoxFuture<'static, (Name, Box<dyn SendData>)>>,
}

impl SimulationRunning {
    #[expect(clippy::too_many_arguments)]
    pub(super) fn new(
        stages: BTreeMap<Name, StageOrAdapter<StageData>>,
        inputs: Inputs,
        effect: EffectBox,
        clock: Arc<dyn Clock + Send + Sync>,
        resources: Resources,
        mailbox_size: usize,
        schedule_ids: ScheduleIds,
        trace_buffer: Arc<Mutex<TraceBuffer>>,
        eval_strategy: Box<dyn EvalStrategy>,
    ) -> Self {
        let (terminate, termination) = watch::channel(false);
        Self {
            stage_count: stages.len(),
            stages,
            inputs,
            effect,
            clock,
            resources,
            runnable: VecDeque::new(),
            scheduled: ScheduledRunnables::new(),
            mailbox_size,
            overrides: Vec::new(),
            breakpoints: Vec::new(),
            schedule_ids,
            trace_buffer,
            eval_strategy,
            terminate,
            termination,
            external_effects: FuturesUnordered::new(),
        }
    }

    /// Get the resources collection for the network.
    ///
    /// This can be used during tests to modify the available resources at specific points in time.
    pub fn resources(&self) -> &Resources {
        &self.resources
    }

    /// Install a breakpoint that will be hit when an effect matching the given predicate is encountered.
    pub fn breakpoint(
        &mut self,
        name: impl AsRef<str>,
        predicate: impl Fn(&Effect) -> bool + Send + 'static,
    ) {
        self.breakpoints
            .push((Name::from(name.as_ref()), Box::new(predicate)));
    }

    /// Remove all breakpoints.
    pub fn clear_breakpoints(&mut self) {
        self.breakpoints.clear();
    }

    /// Remove the breakpoint with the given name.
    pub fn clear_breakpoint(&mut self, name: impl AsRef<str>) {
        self.breakpoints
            .retain(|(n, _)| n.as_str() != name.as_ref());
    }

    /// Install an override for the given external effect type.
    ///
    /// The `remaining` parameter is the number of times the override will be applied
    /// (use `usize::MAX` to apply the override indefinitely).
    /// When the override is applied, the `transform` function is called with the effect
    /// and the result is used to possibly replace the effect.
    ///
    /// If the override result is [`OverrideResult::NoMatch`], the effect is passed to overrides
    /// installed later than this one.
    pub fn override_external_effect<T: ExternalEffect>(
        &mut self,
        remaining: usize,
        mut transform: impl FnMut(Box<T>) -> OverrideResult<Box<T>, Box<dyn ExternalEffect>>
        + Send
        + 'static,
    ) {
        self.overrides.push(OverrideExternalEffect::new(
            remaining,
            Box::new(move |effect| {
                if effect.is::<T>() {
                    // if this casting turns out to be a significant cost, we can split the
                    // overrides by TypeId and run each in an appropriately typed closure
                    #[expect(clippy::expect_used)]
                    match transform(effect.cast::<T>().expect("checked above")) {
                        OverrideResult::NoMatch(effect) => {
                            OverrideResult::NoMatch(effect as Box<dyn ExternalEffect>)
                        }
                        OverrideResult::Handled(msg) => OverrideResult::Handled(msg),
                        OverrideResult::Replaced(effect) => OverrideResult::Replaced(effect),
                    }
                } else {
                    OverrideResult::NoMatch(effect)
                }
            }),
        ));
    }

    /// Get the current simulation time.
    pub fn now(&self) -> Instant {
        self.clock.now()
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
            if self.clock.now() < t {
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

    fn schedule_wakeup(
        &mut self,
        id: ScheduleId,
        wakeup: impl FnOnce(&mut SimulationRunning) + Send + 'static,
    ) {
        self.scheduled.schedule(id, Box::new(wakeup));
    }

    /// Place messages in the given stage’s mailbox, but don’t resume it.
    /// The next message will be consumed when resuming an [`Effect::Receive`]
    /// for this stage.
    ///
    /// # Panics
    ///
    /// Panics if the stage name does not exist (which may also happen due to termination).
    pub fn enqueue_msg<Msg: SendData>(
        &mut self,
        sr: impl AsRef<StageRef<Msg>>,
        msg: impl IntoIterator<Item = Msg>,
    ) {
        for msg in msg.into_iter() {
            let ok = deliver_message(
                &mut self.stages,
                self.mailbox_size,
                sr.as_ref().name().clone(),
                Box::new(msg) as Box<dyn SendData>,
            );
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
        let data = self
            .stages
            .get(sr.as_ref().name())
            .assert_stage("which has no mailbox");
        data.mailbox.len()
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
        let data = self
            .stages
            .get(sr.name())
            .assert_stage("which has no state");
        match &data.state {
            StageState::Idle(state) => {
                Some(state.cast_ref::<St>().expect("internal state type error"))
            }
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

        tracing::debug!(name = %name, "resuming stage");
        self.trace_buffer.lock().push_resume(&name, &response);

        let data = self
            .stages
            .get_mut(&name)
            .assert_stage("which is not runnable");

        let effect = poll_stage(
            &self.trace_buffer,
            &self.schedule_ids,
            data,
            name,
            response,
            &self.effect,
            self.clock.now(),
        );

        if !matches!(effect, Effect::Receive { .. }) {
            self.trace_buffer.lock().push_suspend(&effect);
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
            match deliver_message(
                &mut self.stages,
                self.mailbox_size,
                envelope.name.clone(),
                msg,
            ) {
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
        if self.external_effects.is_empty() {
            return None;
        }
        let (at_stage, result) = select! {
            x = self.external_effects.next() => x?,
            env = self.inputs.next() => {
                self.inputs.put_back(env);
                return None;
            }
        };

        let runnable = &mut self.runnable;
        let run = &mut |name, response| {
            runnable.push_back((name, response));
        };

        let Some(data) = self.stages.get_mut(&at_stage) else {
            tracing::warn!(name = %at_stage, "stage was terminated, skipping external effect delivery");
            return Some(at_stage);
        };
        let data = data.assert_stage("which cannot receive external effects");
        resume_external_internal(data, result, run).expect("external effect is always runnable");
        Some(at_stage)
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
    pub fn run_until_blocked_incl_effects(&mut self, rt: &Handle) -> Blocked {
        loop {
            match self.run_until_sleeping_or_blocked() {
                Blocked::Busy { .. } => {
                    rt.block_on(self.await_external_effect());
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

    /// Run until sleeping or blocked, and if sleeping, skip to the next wakeup.
    /// Return true if the simulation is terminated, false otherwise.
    pub fn run_or_terminated(&mut self) -> bool {
        if self.is_terminated() {
            return true;
        }
        if let Blocked::Sleeping { .. } = self.run_until_sleeping_or_blocked() {
            self.skip_to_next_wakeup(None);
        }
        false
    }

    // TODO: shouldn’t this have a clock ceiling?
    pub fn run_one_step(&mut self, rt: &Handle) -> Option<Blocked> {
        self.receive_inputs();
        match self.run_effect() {
            Some(Blocked::Busy { .. }) => {
                rt.block_on(self.await_external_effect());
                None
            }
            Some(Blocked::Sleeping { .. }) => {
                assert!(self.skip_to_next_wakeup(None));
                None
            }
            other => other,
        }
    }

    fn receive_inputs(&mut self) {
        self.try_inputs();
        let receiving = self
            .stages
            .iter()
            .filter_map(|(n, d)| {
                matches!(
                    d,
                    StageOrAdapter::Stage(StageData {
                        waiting: Some(StageEffect::Receive),
                        ..
                    })
                )
                .then_some(n.clone())
            })
            .collect::<Vec<_>>();
        for name in receiving {
            // ignore all errors since this is a purely optimistic wake-up
            resume_receive_internal(self, &name).ok();
        }
    }

    fn run_effect(&mut self) -> Option<Blocked> {
        let effect = match self.try_effect() {
            Ok(effect) => effect,
            Err(blocked) => return Some(blocked),
        };

        tracing::debug!(runnable = ?self.runnable.iter().map(|r| r.0.as_str()).collect::<Vec<&str>>(), effect = ?effect, "run effect");

        for (name, predicate) in &self.breakpoints {
            if (predicate)(&effect) {
                tracing::info!("breakpoint `{}` hit: {:?}", name, effect);
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
                        let terminated = err
                            .downcast::<resume::UnsupervisedChildTermination>()
                            .map(|e| e.0)
                            .unwrap_or(to);
                        return Some(Blocked::Terminated(terminated));
                    }
                }
                let Some(StageOrAdapter::Stage(data_to)) = self.stages.get_mut(&to) else {
                    return None;
                };
                // resuming receive has removed one message from the mailbox, so check for blocked senders
                let (from, msg) = data_to.senders.pop_front()?;
                post_message(data_to, self.mailbox_size, msg);
                let data_from = self
                    .stages
                    .get_mut(&from)
                    .log_termination(&from)?
                    .assert_stage("which cannot receive send effects");
                resume_send_internal(
                    data_from,
                    &mut |name, response| {
                        tracing::debug!(%name, ?response, "enqueuing stage");
                        self.runnable.push_back((name, response));
                    },
                    to.clone(),
                )
                .expect("call is always runnable");
            }
            Effect::Send { from, to, .. } if to.is_empty() => {
                tracing::info!(stage = %from, "message send to blackhole dropped");
                let data_from = self
                    .stages
                    .get_mut(&from)
                    .log_termination(&from)?
                    .assert_stage("which cannot emit send effects");
                resume_send_internal(data_from, run, to.clone()).expect("call is always runnable");
            }
            Effect::Send { from, to, msg } => {
                let is_call = self
                    .stages
                    .get(&from)
                    .map(|d| {
                        matches!(
                            d,
                            StageOrAdapter::Stage(StageData {
                                waiting: Some(StageEffect::Send(_, Some(_), _)),
                                ..
                            })
                        )
                    })
                    .unwrap_or_default();
                if is_call {
                    // sending stage is always resumed
                    let data_from = self
                        .stages
                        .get_mut(&from)
                        // if the stage was killed while waiting for its turn in sending this response
                        // then the response is simply dropped and the call may time out
                        .log_termination(&from)?
                        .assert_stage("which cannot receive send effects");
                    let id = resume_send_internal(data_from, run, to.clone())
                        .expect("call is always runnable");
                    if let Some(id) = id {
                        self.scheduled.remove(&id);
                    }
                    let data_to = self
                        .stages
                        .get_mut(&to)
                        .log_termination(&to)?
                        .assert_stage("which cannot call");
                    // call response races with other responses and timeout, so failure to resume is okay
                    resume_call_internal(data_to, run, id, msg).ok();
                } else {
                    let mb = self.mailbox_size;
                    let resume = match deliver_message(&mut self.stages, mb, to.clone(), msg) {
                        DeliverMessageResult::Delivered(data_to) => {
                            // `to` may not be suspended on receive, so failure to resume is okay
                            let name = data_to.name.clone();
                            if let Err(err) = resume_receive_internal(self, &name) {
                                tracing::warn!(%from, %to, ?err, "cannot deliver send, shutting down simulation");
                                let terminated = err
                                    .downcast::<resume::UnsupervisedChildTermination>()
                                    .map(|e| e.0)
                                    .unwrap_or(name);
                                return Some(Blocked::Terminated(terminated));
                            }
                            Some(from)
                        }
                        DeliverMessageResult::Full(data_to, send_data) => {
                            data_to.senders.push_back((from, send_data));
                            None
                        }
                        DeliverMessageResult::NotFound => {
                            tracing::warn!(stage = %to, "message send to terminated stage dropped");
                            Some(from)
                        }
                    };
                    if let Some(from) = resume {
                        let data_from = self
                            .stages
                            .get_mut(&from)
                            .log_termination(&from)?
                            .assert_stage("which cannot have sent");
                        resume_send_internal(
                            data_from,
                            &mut |name, response| {
                                tracing::debug!(%name, ?response, "enqueuing stage");
                                self.runnable.push_back((name, response));
                            },
                            to.clone(),
                        )
                        .expect("call is always runnable");
                    }
                }
            }
            Effect::Call {
                from,
                to,
                duration: _,
                msg,
            } => {
                if let Err(err) = resume_call_send_internal(self, from.clone(), to.clone(), msg) {
                    tracing::warn!(%from, %to, %err, "couldn’t deliver call effect");
                    return Some(Blocked::Terminated(from));
                }
            }
            Effect::Clock { at_stage } => {
                let data = self
                    .stages
                    .get_mut(&at_stage)
                    .log_termination(&at_stage)?
                    .assert_stage("which cannot ask for the clock");
                resume_clock_internal(data, run, self.clock.now())
                    .expect("clock effect is always runnable");
            }
            Effect::Wait { at_stage, duration } => {
                let now = self.clock.now();
                let id = self.schedule_ids.next_at(now + duration);
                self.schedule_wakeup(id, move |sim| {
                    let Some(data) = sim.stages.get_mut(&at_stage) else {
                        tracing::warn!(name = %at_stage, "stage was terminated, skipping wait effect delivery");
                        return;
                    };
                    resume_wait_internal(
                        data.assert_stage("which cannot wait"),
                        &mut |name, response| {
                            tracing::debug!(%name, ?response, "enqueuing stage");
                            sim.runnable.push_back((name, response));
                        },
                        sim.clock.now(),
                    )
                        .expect("wait effect is always runnable");
                });
            }
            Effect::Schedule { at_stage, msg, id } => {
                let data = self
                    .stages
                    .get_mut(&at_stage)
                    .log_termination(&at_stage)?
                    .assert_stage("which cannot schedule");
                resume_schedule_internal(data, run, id)
                    .expect("schedule effect is always runnable");
                // Now schedule the wakeup (after run is dropped)
                let now = self.clock.now();
                if id.time() > now {
                    // Schedule wakeup
                    self.schedule_wakeup(id, {
                        move |sim| {
                            let _ =
                                deliver_message(&mut sim.stages, sim.mailbox_size, at_stage, msg);
                        }
                    });
                } else {
                    // Send immediately
                    let _ = deliver_message(&mut self.stages, self.mailbox_size, at_stage, msg);
                }
            }
            Effect::CancelSchedule { at_stage, id } => {
                let cancelled = self.scheduled.remove(&id).is_some();
                let data = self
                    .stages
                    .get_mut(&at_stage)
                    .log_termination(&at_stage)?
                    .assert_stage("which cannot cancel schedule");
                resume_cancel_schedule_internal(data, run, cancelled)
                    .expect("cancel_schedule effect is always runnable");
            }
            Effect::External {
                at_stage,
                mut effect,
            } => {
                let mut result = None;
                for idx in 0..self.overrides.len() {
                    let over = &mut self.overrides[idx];
                    match over.transform(effect) {
                        OverrideResult::NoMatch(effect2) => {
                            effect = effect2;
                        }
                        OverrideResult::Handled(msg) => {
                            result = Some(msg);
                            // dummy effect value since we moved out of `effect` and need it later in the other case
                            effect = Box::new(());
                            if over.register_use_and_get_removal() {
                                self.overrides.remove(idx);
                            }
                            break;
                        }
                        OverrideResult::Replaced(effect2) => {
                            effect = effect2;
                            if over.register_use_and_get_removal() {
                                self.overrides.remove(idx);
                            }
                            break;
                        }
                    }
                }
                if let Some(result) = result {
                    let data = self
                        .stages
                        .get_mut(&at_stage)
                        .log_termination(&at_stage)?
                        .assert_stage("which cannot receive external effects");
                    resume_external_internal(data, result, run)
                        .expect("external effect is always runnable");
                    return None;
                }
                let resources = self.resources.clone();
                self.external_effects.push(Box::pin(async move {
                    (at_stage, effect.run(resources).await)
                }));
            }
            Effect::Terminate { at_stage } => {
                tracing::info!(stage = %at_stage, "terminated");
                let (supervised_by, msg) =
                    self.terminate_stage(at_stage.clone(), TerminationReason::Voluntary)?;
                if supervised_by == *BLACKHOLE_NAME {
                    // top-level stage terminated, terminate the simulation
                    self.terminate.send_replace(true);
                    return Some(Blocked::Terminated(at_stage));
                }
                let supervisor = self
                    .stages
                    .get_mut(&supervised_by)
                    .assert_stage("which cannot supervise");
                supervisor.tombstones.push_back(msg);
                if let Err(err) = resume_receive_internal(self, &supervised_by) {
                    tracing::warn!(%supervised_by, ?err, "shutting down simulation");
                    let terminated = err
                        .downcast::<resume::UnsupervisedChildTermination>()
                        .map(|e| e.0)
                        .unwrap_or(supervised_by);
                    return Some(Blocked::Terminated(terminated));
                }
            }
            Effect::AddStage { at_stage, name } => {
                let name = stage_name(&mut self.stage_count, name.as_str());
                let data = self
                    .stages
                    .get_mut(&at_stage)
                    .log_termination(&at_stage)?
                    .assert_stage("which cannot add a stage");
                resume_add_stage_internal(data, run, name)
                    .expect("add stage effect is always runnable");
            }
            Effect::WireStage {
                at_stage,
                name,
                initial_state,
                tombstone,
            } => {
                self.trace_buffer.lock().push_state(&name, &initial_state);
                let data = self
                    .stages
                    .get_mut(&at_stage)
                    .log_termination(&at_stage)?
                    .assert_stage("which cannot wire a stage");
                let transition = resume_wire_stage_internal(data, run)
                    .expect("wire stage effect is always runnable");
                let tombstone = tombstone.try_cast::<CanSupervise>().err();
                self.stages.insert(
                    name.clone(),
                    StageOrAdapter::Stage(StageData {
                        name,
                        mailbox: VecDeque::new(),
                        tombstones: VecDeque::new(),
                        state: StageState::Idle(initial_state),
                        transition: (transition)(self.effect.clone()),
                        waiting: Some(StageEffect::Receive),
                        senders: VecDeque::new(),
                        supervised_by: at_stage,
                        tombstone,
                    }),
                );
            }
            Effect::Contramap {
                at_stage,
                original,
                new_name,
            } => {
                let name = stage_name(&mut self.stage_count, new_name.as_str());
                let data = self
                    .stages
                    .get_mut(&at_stage)
                    .assert_stage("which cannot call contramap");
                let transform =
                    resume_contramap_internal(data, run, original.clone(), name.clone())
                        .expect("contramap effect is always runnable");
                self.stages.insert(
                    name.clone(),
                    StageOrAdapter::Adapter(Adapter {
                        name,
                        target: original,
                        transform,
                    }),
                );
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
        let data = data.assert_stage("which cannot terminate");

        // parent state is dropped before the children, but pure-stage states are just dumb data
        // anyway, so this should usually be what we want
        data.state = StageState::Terminating;

        // clean up simulation state for this stage
        self.runnable.retain(|(n, _)| n != &at_stage);

        let runnable = &mut self.runnable;
        let run = &mut |name, response| {
            tracing::debug!(%name, ?response, "enqueuing stage");
            runnable.push_back((name, response));
        };
        let senders = std::mem::take(&mut data.senders);
        for (waiting, _) in senders {
            let data = self
                .stages
                .get_mut(&waiting)
                .assert_stage("which cannot send");
            if let Err(err) = resume_send_internal(data, run, at_stage.clone()) {
                tracing::error!(from = %waiting, to = %at_stage, %err, "failed to resume send");
                continue;
            };
        }

        let children = self
            .stages
            .iter()
            .filter(|(_, d)| {
                matches!(d, StageOrAdapter::Stage(StageData { supervised_by, .. })
                    if supervised_by == &at_stage)
            })
            .map(|(n, _)| n.clone())
            .collect::<Vec<_>>();
        for child in children {
            tracing::info!(stage = %child, parent = %at_stage, "terminating child stage");
            self.terminate_stage(child, TerminationReason::Aborted);
        }
        self.trace_buffer.lock().push_terminated(&at_stage, reason);
        let Some(StageOrAdapter::Stage(stage)) = self.stages.remove(&at_stage) else {
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
            let StageOrAdapter::Stage(data) = data else {
                if self.runnable.iter().any(|(n, _)| n == name) {
                    panic!("stage `{name}` is runnable but is an adapter");
                }
                continue;
            };
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
    pub fn resume_receive<Msg>(
        &mut self,
        at_stage: impl AsRef<StageRef<Msg>>,
    ) -> anyhow::Result<()> {
        resume_receive_internal(self, at_stage.as_ref().name()).and_then(|resumed| {
            if resumed {
                Ok(())
            } else {
                Err(anyhow::anyhow!(
                    "stage was not waiting for a receive effect"
                ))
            }
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
            && deliver_message(
                &mut self.stages,
                self.mailbox_size,
                to.name().clone(),
                Box::new(msg),
            )
            .is_full()
        {
            anyhow::bail!("mailbox is full while resuming send");
        }

        let data = self
            .stages
            .get_mut(from.as_ref().name())
            .assert_stage("which cannot send");
        let id = resume_send_internal(
            data,
            &mut |name, response| {
                tracing::debug!(%name, ?response, "enqueuing stage");
                self.runnable.push_back((name, response));
            },
            to.name().clone(),
        )?;

        if let Some(id) = id
            && let Some(msg) = msg
        {
            self.scheduled.remove(&id);
            let data = self
                .stages
                .get_mut(to.name())
                .assert_stage("which cannot call");
            resume_call_internal(
                data,
                &mut |name, response| {
                    tracing::debug!(%name, ?response, "enqueuing stage");
                    self.runnable.push_back((name, response));
                },
                Some(id),
                Box::new(msg),
            )?;
        }

        Ok(())
    }

    /// Resume an [`Effect::Clock`].
    ///
    /// # Panics
    ///
    /// Panics if the stage name does not exist (which may also happen due to termination).
    pub fn resume_clock<Msg>(
        &mut self,
        at_stage: impl AsRef<StageRef<Msg>>,
        time: Instant,
    ) -> anyhow::Result<()> {
        let data = self
            .stages
            .get_mut(at_stage.as_ref().name())
            .assert_stage("which cannot ask for the clock");
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
    pub fn resume_wait<Msg>(
        &mut self,
        at_stage: impl AsRef<StageRef<Msg>>,
        time: Instant,
    ) -> anyhow::Result<()> {
        let data = self
            .stages
            .get_mut(at_stage.as_ref().name())
            .assert_stage("which cannot wait");
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
        resume_call_send_internal(
            self,
            from.as_ref().name().clone(),
            to.as_ref().name().clone(),
            Box::new(msg),
        )
        .and_then(|resumed| {
            if resumed {
                Ok(())
            } else {
                Err(anyhow::anyhow!("stage was not waiting for a call effect"))
            }
        })
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
        let data = self
            .stages
            .get_mut(at_stage.name())
            .assert_stage("which cannot make a call");
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
    /// # Panics
    ///
    /// Panics if the stage name does not exist (which may also happen due to termination).
    pub fn resume_external_box(
        &mut self,
        at_stage: impl AsRef<Name>,
        result: Box<dyn SendData>,
    ) -> anyhow::Result<()> {
        let data = self
            .stages
            .get_mut(at_stage.as_ref())
            .assert_stage("which cannot receive external effects");
        resume_external_internal(data, result, &mut |name, response| {
            tracing::debug!(%name, ?response, "enqueuing stage");
            self.runnable.push_back((name, response));
        })
    }

    /// Resume an [`Effect::External`].
    ///
    /// # Panics
    ///
    /// Panics if the stage name does not exist (which may also happen due to termination).
    pub fn resume_external<Eff: ExternalEffectAPI>(
        &mut self,
        at_stage: impl AsRef<Name>,
        result: Eff::Response,
    ) -> anyhow::Result<()> {
        let data = self
            .stages
            .get_mut(at_stage.as_ref())
            .assert_stage("which cannot receive external effects");
        resume_external_internal(data, Box::new(result), &mut |name, response| {
            tracing::debug!(%name, ?response, "enqueuing stage");
            self.runnable.push_back((name, response));
        })
    }

    /// Resume an [`Effect::AddStage`].
    ///
    /// # Panics
    ///
    /// Panics if the stage name does not exist (which may also happen due to termination).
    pub fn resume_add_stage<Msg>(
        &mut self,
        at_stage: impl AsRef<StageRef<Msg>>,
        name: Name,
    ) -> anyhow::Result<()> {
        let data = self
            .stages
            .get_mut(at_stage.as_ref().name())
            .assert_stage("which cannot add a stage");
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
        let data = self
            .stages
            .get_mut(at_stage.name())
            .assert_stage("which cannot wire a stage");
        let transition = resume_wire_stage_internal(data, &mut |name, response| {
            tracing::debug!(%name, ?response, "enqueuing stage");
            self.runnable.push_back((name, response));
        })?;

        self.stages.insert(
            name.clone(),
            StageOrAdapter::Stage(StageData {
                name,
                mailbox: VecDeque::new(),
                tombstones: VecDeque::new(),
                state: StageState::Idle(initial_state),
                transition: (transition)(self.effect.clone()),
                waiting: Some(StageEffect::Receive),
                senders: VecDeque::new(),
                supervised_by: at_stage.name().clone(),
                tombstone,
            }),
        );
        Ok(())
    }

    /// Resume an [`Effect::Contramap`].
    pub fn resume_contramap<Msg>(
        &mut self,
        at_stage: impl AsRef<StageRef<Msg>>,
        original: Name,
        name: Name,
    ) -> anyhow::Result<()> {
        let data = self
            .stages
            .get_mut(at_stage.as_ref().name())
            .assert_stage("which cannot contramap");
        let transform = resume_contramap_internal(
            data,
            &mut |name, response| {
                tracing::debug!(%name, ?response, "enqueuing stage");
                self.runnable.push_back((name, response));
            },
            original.clone(),
            name.clone(),
        )?;
        self.stages.insert(
            name.clone(),
            StageOrAdapter::Adapter(Adapter {
                name,
                target: original,
                transform,
            }),
        );
        Ok(())
    }
}

trait AssertStage<'a> {
    type Output: 'a;
    fn assert_stage(self, hint: &'static str) -> Self::Output
    where
        Self: 'a;
}
impl<'a> AssertStage<'a> for &'a mut StageOrAdapter<StageData> {
    type Output = &'a mut StageData;
    fn assert_stage(self, hint: &'static str) -> Self::Output {
        match self {
            StageOrAdapter::Stage(stage) => stage,
            StageOrAdapter::Adapter(_) => {
                panic!("stage is an adapter, {hint}")
            }
        }
    }
}
impl<'a> AssertStage<'a> for Option<&'a mut StageOrAdapter<StageData>> {
    type Output = &'a mut StageData;
    fn assert_stage(self, hint: &'static str) -> Self::Output {
        let this = match self {
            Some(this) => this,
            None => panic!("stage not found"),
        };
        match this {
            StageOrAdapter::Stage(stage) => stage,
            StageOrAdapter::Adapter(_) => {
                panic!("stage is an adapter, {hint}")
            }
        }
    }
}
impl<'a> AssertStage<'a> for Option<&'a StageOrAdapter<StageData>> {
    type Output = &'a StageData;
    fn assert_stage(self, hint: &'static str) -> Self::Output {
        let this = match self {
            Some(this) => this,
            None => panic!("stage not found"),
        };
        match this {
            StageOrAdapter::Stage(stage) => stage,
            StageOrAdapter::Adapter(_) => {
                panic!("stage is an adapter, {hint}")
            }
        }
    }
}

trait LogTermination<'a> {
    type Output: 'a;
    fn log_termination(self, name: &Name) -> Self::Output
    where
        Self: 'a;
}
impl<'a> LogTermination<'a> for Option<&'a mut StageOrAdapter<StageData>> {
    type Output = Option<&'a mut StageOrAdapter<StageData>>;
    fn log_termination(self, name: &Name) -> Self::Output {
        if self.is_none() {
            tracing::warn!(%name, "stage was terminated, skipping effect handling");
        }
        self
    }
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

// module to make fields actually private
mod override_external_effect {
    use super::*;

    pub struct OverrideExternalEffect {
        remaining: usize,
        transform: Box<
            dyn FnMut(
                    Box<dyn ExternalEffect>,
                )
                    -> OverrideResult<Box<dyn ExternalEffect>, Box<dyn ExternalEffect>>
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
                dyn FnMut(
                        Box<dyn ExternalEffect>,
                    )
                        -> OverrideResult<Box<dyn ExternalEffect>, Box<dyn ExternalEffect>>
                    + Send
                    + 'static,
            >,
        ) -> Self {
            Self {
                remaining,
                transform,
            }
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
    if sim
        .stages
        .values()
        .filter_map(|d| d.as_stage().and_then(|d| d.waiting.as_ref()))
        .all(|v| matches!(v, StageEffect::Receive))
    {
        if let Some(next_wakeup) = sim.next_wakeup() {
            return Blocked::Sleeping { next_wakeup };
        } else {
            return Blocked::Idle;
        }
    }
    let mut send = Vec::new();
    let mut busy = Vec::new();
    let mut sleep = Vec::new();
    for (k, v) in sim.stages.iter().filter_map(|(k, d)| {
        d.as_stage()
            .and_then(|d| d.waiting.as_ref())
            .map(|w| (k, w))
    }) {
        match v {
            StageEffect::Send(name, None, _msg) => send.push(SendBlock {
                from: k.clone(),
                to: name.clone(),
                is_call: false,
            }),
            StageEffect::Receive => {}
            StageEffect::Wait(..) => sleep.push(k.clone()),
            StageEffect::Call(_, _, CallExtra::Scheduled(id)) if sim.scheduled.contains(id) => {
                sleep.push(k.clone())
            }
            _ => busy.push(k.clone()),
        }
    }

    if !busy.is_empty() {
        Blocked::Busy {
            stages: busy,
            external_effects: sim.external_effects.len(),
        }
    } else if !sleep.is_empty() {
        Blocked::Sleeping {
            next_wakeup: sim
                .next_wakeup()
                .expect("stages are waiting for a wait effect"),
        }
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
        panic!(
            "runnable stage `{name}` is not running but {:?}",
            data.state
        );
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

/// Deliver a message to a stage or adapter.
///
/// Returns `true` if the message was delivered, `false` if the target mailbox
/// does not exist, or `Err` if the mailbox is full.
fn deliver_message(
    stages: &mut BTreeMap<Name, StageOrAdapter<StageData>>,
    mailbox_size: usize,
    name: Name,
    msg: Box<dyn SendData>,
) -> DeliverMessageResult<'_> {
    let Some((data, msg)) = find_recipient(stages, name, Some(msg)) else {
        return DeliverMessageResult::NotFound;
    };

    post_message(data, mailbox_size, msg)
}

fn post_message(
    data: &mut StageData,
    mailbox_size: usize,
    msg: Box<dyn SendData>,
) -> DeliverMessageResult<'_> {
    if data.mailbox.len() >= mailbox_size {
        return DeliverMessageResult::Full(data, msg);
    }
    data.mailbox.push_back(msg);
    DeliverMessageResult::Delivered(data)
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
        eff.call(&eff.me(), std::time::Duration::from_secs(1), |cr| {
            Msg(Some(cr))
        })
        .await;
        true
    });

    let stage = network.wire_up(stage, false);
    let mut sim = network.run();

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
        let effect = if idx == 0 {
            Effect::Receive {
                at_stage: "stage".into(),
            }
        } else {
            sim.effect()
        };
        tracing::info!(effect = ?effect, "effect");
        assert!(
            ops[idx].0(&effect),
            "effect {effect:?} should match predicate for `{idx}`"
        );
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
