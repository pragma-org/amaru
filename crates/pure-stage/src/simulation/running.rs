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

#[cfg(test)]
use crate::simulation::SimulationBuilder;
use crate::{
    BoxFuture, CallId, Effect, ExternalEffect, ExternalEffectAPI, Instant, Name, Resources,
    SendData, StageRef, StageResponse,
    adapter::{StageOrAdapter, find_recipient},
    effect::StageEffect,
    effect_box::EffectBox,
    simulation::{
        blocked::{Blocked, SendBlock},
        inputs::Inputs,
        random::EvalStrategy,
        resume::{
            resume_add_stage_internal, resume_call_internal, resume_clock_internal,
            resume_external_internal, resume_receive_internal, resume_respond_internal,
            resume_send_internal, resume_wait_internal, resume_wire_stage_internal,
        },
        state::{StageData, StageState},
    },
    stage_name,
    stage_ref::StageStateRef,
    stagegraph::{CallRef, StageGraphRunning},
    time::Clock,
    trace_buffer::TraceBuffer,
};
use either::Either::{Left, Right};
use futures_util::{StreamExt, stream::FuturesUnordered};
use parking_lot::Mutex;
use std::{
    collections::{BTreeMap, BinaryHeap, VecDeque},
    mem::{replace, take},
    sync::Arc,
    task::{Context, Poll, Waker},
    time::Duration,
};
use tokio::{
    runtime::Handle,
    select,
    sync::{oneshot, watch},
};

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
    sleeping: BinaryHeap<Sleeping>,
    responded: Vec<(Name, CallId)>,
    mailbox_size: usize,
    overrides: Vec<OverrideExternalEffect>,
    breakpoints: Vec<(Name, Box<dyn Fn(&Effect) -> bool + Send + 'static>)>,
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
            sleeping: BinaryHeap::new(),
            responded: Vec::new(),
            mailbox_size,
            overrides: Vec::new(),
            breakpoints: Vec::new(),
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
        self.overrides.push(OverrideExternalEffect {
            remaining,
            transform: Box::new(move |effect| {
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
        });
    }

    /// Get the current simulation time.
    pub fn now(&self) -> Instant {
        self.clock.now()
    }

    /// Advance the clock to the next wakeup time.
    ///
    /// Returns `true` if wakeups were performed, `false` if there are no more wakeups or
    /// the clock was advanced to the given `max_time`.
    pub fn skip_to_next_wakeup(&mut self, max_time: Option<Instant>) -> bool {
        let Some(Sleeping { time, .. }) = self.sleeping.peek() else {
            if let Some(time) = max_time {
                self.clock.advance_to(time);
                self.trace_buffer.lock().push_clock(time);
            }
            return false;
        };

        // only advance as far as allowed
        let time = (*time).min(max_time.unwrap_or(*time));

        self.clock.advance_to(time);
        self.trace_buffer.lock().push_clock(time);

        let mut performed_wakeups = false;

        // this won't find a match if max_time was hit
        while matches!(self.sleeping.peek(), Some(Sleeping { time: t, .. }) if *t == time) {
            let Sleeping { wakeup, .. } = self.sleeping.pop().expect("peeked, so must exist");
            wakeup(self);
            performed_wakeups = true;
        }
        performed_wakeups
    }

    pub fn next_wakeup(&self) -> Option<Instant> {
        self.sleeping.peek().map(|Sleeping { time, .. }| *time)
    }

    fn schedule_wakeup(
        &mut self,
        duration: Duration,
        call_id: Option<CallId>,
        wakeup: impl FnOnce(&mut SimulationRunning) + Send + 'static,
    ) {
        assert!(
            duration > Duration::ZERO,
            "cannot schedule wakeup with zero delay"
        );
        let time = self.clock.now() + duration;
        self.sleeping.push(Sleeping {
            time,
            call_id,
            wakeup: Box::new(wakeup),
        });
    }

    /// Place messages in the given stage’s mailbox, but don’t resume it.
    /// The next message will be consumed when resuming an [`Effect::Receive`]
    /// for this stage.
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
        let (name, response) = if self.runnable.is_empty() {
            let reason = block_reason(self);
            tracing::debug!("blocking for reason: {:?}", reason);
            return Err(reason);
        } else {
            self.eval_strategy.pick_runnable(&mut self.runnable)
        };

        tracing::debug!(name = %name, "resuming stage");
        self.trace_buffer.lock().push_resume(&name, &response);

        let data = self
            .stages
            .get_mut(&name)
            .assert_stage("which is not runnable");

        let effect = poll_stage(
            &self.trace_buffer,
            data,
            name.clone(),
            response,
            &self.effect,
        );

        let names = take(&mut self.responded);
        let runnable = &mut self.runnable;
        let run = &mut |name, response| {
            runnable.push_back((name, response));
        };
        if !names.is_empty() {
            self.sleeping.retain(|s| {
                let Some(cid) = s.call_id else {
                    return true;
                };
                names.iter().all(|(_, id)| cid != *id)
            });
        }
        for (name, id) in names {
            let Some((recip, _)) = find_recipient(&mut self.stages, name, None) else {
                continue; // responding to CallRef::channel()
            };
            let name = recip.name.clone();
            let data = self
                .stages
                .get_mut(&name)
                .assert_stage("which does not receive call responses");
            // just trying to resume as far as possible, so failure to resume is okay
            resume_call_internal(data, run, id).ok();
        }

        self.trace_buffer.lock().push_suspend(&effect);

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

        let data = self
            .stages
            .get_mut(&at_stage)
            .assert_stage("which cannot receive external effects");
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

    // FIXME: shouldn’t this have a clock ceiling?
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

        {
            let runnable = &mut self.runnable;
            let run = &mut |name, response| {
                runnable.push_back((name, response));
            };
            for data in self.stages.values_mut() {
                let StageOrAdapter::Stage(data) = data else {
                    continue;
                };
                let Some(StageEffect::Receive) = &data.waiting else {
                    continue;
                };
                resume_receive_internal(&mut self.trace_buffer.lock(), data, run).ok();
            }
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
            runnable.push_back((name, response));
        };

        match effect {
            Effect::Receive { at_stage: to } => {
                let data_to = self
                    .stages
                    .get_mut(&to)
                    .assert_stage("which cannot emit receive effects");
                resume_receive_internal(&mut self.trace_buffer.lock(), data_to, run).ok()?;
                // resuming receive has removed one message from the mailbox, so check for blocked senders
                let (from, msg) = data_to.senders.pop_front()?;
                post_message(data_to, self.mailbox_size, msg);
                let data_from = self
                    .stages
                    .get_mut(&from)
                    .assert_stage("which cannot receive send effects");
                let call = resume_send_internal(data_from, run, to.clone())
                    .expect("call is always runnable");
                self.handle_call_continuation(from, to, call);
            }
            Effect::Send {
                from,
                to,
                msg,
                call: _,
            } => {
                if to.as_str().is_empty() {
                    tracing::warn!(stage = %from, "message send to blackhole dropped");
                    let data_from = self
                        .stages
                        .get_mut(&from)
                        .assert_stage("which cannot emit send effects");
                    let call = resume_send_internal(data_from, run, to.clone())
                        .expect("call is always runnable");
                    self.handle_call_continuation(from, to, call);
                    return None;
                }

                let resume =
                    match deliver_message(&mut self.stages, self.mailbox_size, to.clone(), msg) {
                        DeliverMessageResult::Delivered(data_to) => {
                            // `to` may not be suspended on receive, so failure to resume is okay
                            resume_receive_internal(&mut self.trace_buffer.lock(), data_to, run)
                                .ok();
                            Some(from)
                        }
                        DeliverMessageResult::Full(data_to, send_data) => {
                            data_to.senders.push_back((from, send_data));
                            None
                        }
                        DeliverMessageResult::NotFound => {
                            tracing::warn!(stage = %to, "message send to terminatedstage dropped");
                            Some(from)
                        }
                    };
                if let Some(from) = resume {
                    let data_from = self
                        .stages
                        .get_mut(&from)
                        .assert_stage("which cannot have sent");
                    let call = resume_send_internal(data_from, run, to.clone())
                        .expect("call is always runnable");
                    self.handle_call_continuation(from, to, call);
                }
            }
            Effect::Clock { at_stage } => {
                let data = self
                    .stages
                    .get_mut(&at_stage)
                    .assert_stage("which cannot ask for the clock");
                Self::resume_clock_internal(data, run, self.clock.now())
                    .expect("clock effect is always runnable");
            }
            Effect::Wait { at_stage, duration } => {
                self.schedule_wakeup(duration, None, move |sim| {
                    let data = sim
                        .stages
                        .get_mut(&at_stage)
                        .assert_stage("which cannot wait");
                    resume_wait_internal(
                        data,
                        &mut |name, response| {
                            sim.runnable.push_back((name, response));
                        },
                        sim.clock.now(),
                    )
                    .expect("wait effect is always runnable");
                });
            }
            Effect::Respond {
                at_stage,
                target,
                id,
                msg,
            } => {
                let data = self
                    .stages
                    .get_mut(&at_stage)
                    .assert_stage("which cannot respond");
                let res = resume_respond_internal(data, run, target, id)
                    .expect("respond effect is always runnable");
                self.handle_send_response(msg, res);
            }
            Effect::External {
                at_stage,
                mut effect,
            } => {
                let mut result = None;
                for idx in 0..self.overrides.len() {
                    let over = &mut self.overrides[idx];
                    match (over.transform)(effect) {
                        OverrideResult::NoMatch(effect2) => {
                            effect = effect2;
                        }
                        OverrideResult::Handled(msg) => {
                            result = Some(msg);
                            // dummy effect value since we moved out of `effect` and need it later in the other case
                            effect = Box::new(());
                            over.remaining -= 1;
                            if over.remaining == 0 {
                                self.overrides.remove(idx);
                            }
                            break;
                        }
                        OverrideResult::Replaced(effect2) => {
                            effect = effect2;
                            over.remaining -= 1;
                            if over.remaining == 0 {
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
                self.terminate.send_replace(true);
                return Some(Blocked::Terminated(at_stage));
            }
            Effect::AddStage { at_stage, name } => {
                let name = stage_name(&mut self.stage_count, name.as_str());
                let data = self
                    .stages
                    .get_mut(&at_stage)
                    .assert_stage("which cannot add a stage");
                resume_add_stage_internal(data, run, name)
                    .expect("add stage effect is always runnable");
            }
            Effect::WireStage {
                at_stage,
                name,
                initial_state,
            } => {
                let data = self
                    .stages
                    .get_mut(&at_stage)
                    .assert_stage("which cannot wire a stage");
                let transition = resume_wire_stage_internal(data, run)
                    .expect("wire stage effect is always runnable");
                self.stages.insert(
                    name.clone(),
                    StageOrAdapter::Stage(StageData {
                        name,
                        mailbox: VecDeque::new(),
                        state: StageState::Idle(initial_state),
                        transition: (transition)(self.effect.clone()),
                        waiting: Some(StageEffect::Receive),
                        senders: VecDeque::new(),
                    }),
                );
            }
        }
        None
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
                StageState::Failed(_) => {
                    if waiting.is_some() {
                        panic!("stage `{name}` is Failed but waiting for {waiting:?}");
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
        let data = self
            .stages
            .get_mut(at_stage.as_ref().name())
            .assert_stage("which cannot receive");
        resume_receive_internal(
            &mut self.trace_buffer.lock(),
            data,
            &mut |name, response| {
                self.runnable.push_back((name, response));
            },
        )
    }

    /// Resume an [`Effect::Send`].
    pub fn resume_send<Msg1, Msg2: SendData>(
        &mut self,
        from: impl AsRef<StageRef<Msg1>>,
        to: impl AsRef<StageRef<Msg2>>,
        msg: Option<Msg2>,
    ) -> anyhow::Result<()> {
        let to = to.as_ref();
        if let Some(msg) = msg
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
        let call = resume_send_internal(
            data,
            &mut |name, response| {
                self.runnable.push_back((name, response));
            },
            to.name().clone(),
        )?;

        self.handle_call_continuation(from.as_ref().name().clone(), to.name().clone(), call);
        Ok(())
    }

    fn handle_call_continuation(
        &mut self,
        from: Name,
        to: Name,
        call: Option<(
            Duration,
            oneshot::Receiver<Box<dyn SendData + 'static>>,
            CallId,
        )>,
    ) {
        if let Some((timeout, recv, id)) = call {
            let deadline = self.clock.now() + timeout;
            self.stages
                .get_mut(&from)
                .assert_stage("which cannot make a call")
                .waiting = Some(StageEffect::Call(to, deadline, (), recv, id));
            self.schedule_wakeup(timeout, Some(id), move |sim| {
                let data = sim
                    .stages
                    .get_mut(&from)
                    .assert_stage("which cannot make a call");
                resume_call_internal(
                    data,
                    &mut |name, response| {
                        sim.runnable.push_back((name, response));
                    },
                    id,
                )
                .ok();
            });
        }
    }

    /// Resume an [`Effect::Clock`].
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
                self.runnable.push_back((name, response));
            },
            time,
        )
    }

    fn resume_clock_internal(
        data: &mut StageData,
        run: &mut dyn FnMut(Name, StageResponse),
        time: Instant,
    ) -> anyhow::Result<()> {
        let waiting_for = data.waiting.as_ref().ok_or_else(|| {
            anyhow::anyhow!("stage `{}` was not waiting for any effect", data.name)
        })?;

        if !matches!(waiting_for, StageEffect::Clock) {
            anyhow::bail!(
                "stage `{}` was not waiting for a clock effect, but {:?}",
                data.name,
                waiting_for
            )
        }

        // it is important that all validations (i.e. `?``) happen before this point
        data.waiting = None;

        run(data.name.clone(), StageResponse::ClockResponse(time));
        Ok(())
    }

    /// Resume an [`Effect::Wait`].
    ///
    /// The given time is the clock when the stage wakes up.
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
                self.runnable.push_back((name, response));
            },
            time,
        )
    }

    /// Resume an [`Effect::Send`]’s second stage in case of a call.
    ///
    /// The message to be delivered to the stage must have been sent by the called stage already.
    pub fn resume_call<Msg, Resp: SendData>(
        &mut self,
        at_stage: impl AsRef<StageRef<Msg>>,
        call: &CallRef<Resp>,
    ) -> anyhow::Result<()> {
        let data = self
            .stages
            .get_mut(at_stage.as_ref().name())
            .assert_stage("which cannot make a call");
        resume_call_internal(
            data,
            &mut |name, response| {
                self.runnable.push_back((name, response));
            },
            call.id,
        )
    }

    /// Resume an [`Effect::Respond`].
    pub fn resume_respond<Msg, Resp: SendData>(
        &mut self,
        at_stage: impl AsRef<StageRef<Msg>>,
        cr: &CallRef<Resp>,
        msg: Resp,
    ) -> anyhow::Result<()> {
        let data = self
            .stages
            .get_mut(at_stage.as_ref().name())
            .assert_stage("which cannot respond");
        let res = resume_respond_internal(
            data,
            &mut |name, response| {
                self.runnable.push_back((name, response));
            },
            cr.target.clone(),
            cr.id,
        )?;

        self.handle_send_response(Box::new(msg), res);
        Ok(())
    }

    fn handle_send_response(
        &mut self,
        msg: Box<dyn SendData>,
        (target, id, deadline, sender): (Name, CallId, Instant, oneshot::Sender<Box<dyn SendData>>),
    ) {
        if let Err(msg) = sender.send(msg) {
            tracing::warn!(
                "response to {} was dropped: {:?} (deadline: {})",
                target,
                msg,
                deadline.pretty(self.now())
            );
        } else {
            self.responded.push((target, id));
        }
    }

    /// Resume an [`Effect::External`].
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
            self.runnable.push_back((name, response));
        })
    }

    /// Resume an [`Effect::External`].
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
            self.runnable.push_back((name, response));
        })
    }

    /// Resume an [`Effect::AddStage`].
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
                self.runnable.push_back((name, response));
            },
            name,
        )
    }

    /// Resume an [`Effect::WireStage`].
    pub fn resume_wire_stage<Msg>(
        &mut self,
        at_stage: impl AsRef<StageRef<Msg>>,
        name: Name,
        initial_state: Box<dyn SendData>,
    ) -> anyhow::Result<()> {
        let data = self
            .stages
            .get_mut(at_stage.as_ref().name())
            .assert_stage("which cannot wire a stage");
        let transition = resume_wire_stage_internal(data, &mut |name, response| {
            self.runnable.push_back((name, response));
        })?;

        self.stages.insert(
            name.clone(),
            StageOrAdapter::Stage(StageData {
                name,
                mailbox: VecDeque::new(),
                state: StageState::Idle(initial_state),
                transition: (transition)(self.effect.clone()),
                waiting: Some(StageEffect::Receive),
                senders: VecDeque::new(),
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

/// An entry for the sleeping stage heap.
///
/// NOTE: the `Ord` implementation is reversed, so that the heap is a min-heap.
/// The `wakeup` is secondarily ordered according to the address of the closure.
struct Sleeping {
    time: Instant,
    call_id: Option<CallId>,
    wakeup: Box<dyn FnOnce(&mut SimulationRunning) + Send + 'static>,
}

impl std::fmt::Debug for Sleeping {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sleeping")
            .field("time", &self.time)
            .finish()
    }
}

impl Eq for Sleeping {}

impl Ord for Sleeping {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.time.cmp(&other.time).reverse().then_with(|| {
            let left = self.wakeup.as_ref() as *const _ as *const u8 as usize;
            let right = other.wakeup.as_ref() as *const _ as *const u8 as usize;
            left.cmp(&right)
        })
    }
}

impl PartialEq for Sleeping {
    fn eq(&self, other: &Self) -> bool {
        self.time == other.time && std::ptr::eq(self.wakeup.as_ref(), other.wakeup.as_ref())
    }
}

impl PartialOrd for Sleeping {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

struct OverrideExternalEffect {
    remaining: usize,
    transform: Box<
        dyn FnMut(
                Box<dyn ExternalEffect>,
            ) -> OverrideResult<Box<dyn ExternalEffect>, Box<dyn ExternalEffect>>
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
            StageEffect::Send(name, _msg, call) => send.push(SendBlock {
                from: k.clone(),
                to: name.clone(),
                is_call: call.is_some(),
            }),
            StageEffect::Receive => {}
            StageEffect::Wait(..) | StageEffect::Call(..) => sleep.push(k.clone()),
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
    data: &mut StageData,
    name: Name,
    response: StageResponse,
    effect: &EffectBox,
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
        let (wait_effect, effect) = stage_effect.split(name.clone());
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
    use crate::{StageGraph, stagegraph::CallRef};

    tracing_subscriber::fmt()
        .with_test_writer()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init()
        .ok();

    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    struct Msg(Option<CallRef<()>>);

    let mut network = SimulationBuilder::default();
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
        Box<dyn Fn(&Effect) -> Option<CallId>>,
        Box<dyn Fn(&mut SimulationRunning, &StageRef<Msg>, CallId) -> anyhow::Result<()>>,
        &'static str,
    ); 5] = [
        (
            Box::new(|eff: &Effect| {
                matches!(eff, Effect::Receive { .. }).then(|| CallId::from_u64(0))
            }),
            Box::new(|sim, stage, _id| sim.resume_receive(stage)),
            "resume_receive",
        ),
        (
            Box::new(|eff: &Effect| {
                // note that this also matches in the Call case, which is correct;
                // resume_send will advance the stage to await the response
                matches!(eff, Effect::Send { .. }).then(|| CallId::from_u64(1))
            }),
            Box::new(|sim, stage, _id| sim.resume_send(stage, stage, Some(Msg(None)))),
            "resume_send",
        ),
        (
            Box::new(|eff: &Effect| {
                matches!(eff, Effect::Clock { .. }).then(|| CallId::from_u64(2))
            }),
            Box::new(|sim, stage, _id| sim.resume_clock(stage, Instant::now())),
            "resume_clock",
        ),
        (
            Box::new(|eff: &Effect| {
                matches!(eff, Effect::Wait { .. }).then(|| CallId::from_u64(3))
            }),
            Box::new(|sim, stage, _id| sim.resume_wait(stage, Instant::now())),
            "resume_wait",
        ),
        (
            Box::new(|eff: &Effect| match eff {
                Effect::Send {
                    msg, call: Some(_), ..
                } => Some(
                    msg.cast_ref::<Msg>()
                        .expect("internal message type error")
                        .0
                        .as_ref()
                        .unwrap()
                        .id,
                ),
                _ => None,
            }),
            // resume_call works because resume_send from the second item has already been called
            Box::new(|sim, stage, id| {
                let data = sim
                    .stages
                    .get_mut(stage.name())
                    .assert_stage("which cannot make a call");
                resume_call_internal(
                    data,
                    &mut |name, response| {
                        sim.runnable.push_back((name, response));
                    },
                    id,
                )
            }),
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
            ops[idx].0(&effect).is_some(),
            "effect {effect:?} should match predicate for `{idx}`"
        );
        for (pred, op, name) in &ops {
            if pred(&effect).is_none() {
                tracing::info!("op `{}` should not work", name);
                op(
                    &mut sim,
                    &stage.clone().without_state(),
                    CallId::from_u64(0),
                )
                .unwrap_err();
                sim.invariants();
            }
        }
        for (pred, op, name) in &ops {
            if let Some(id) = pred(&effect) {
                tracing::info!("op `{}` should work", name);
                op(&mut sim, &stage.clone().without_state(), id).unwrap();
                sim.invariants();
            }
        }
    }
    tracing::info!("final invariants");
    sim.effect().assert_receive(&stage);
    let state = sim.get_state(&stage).unwrap();
    assert!(state);
}
