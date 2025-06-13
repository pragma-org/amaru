use super::{
    inputs::Inputs, EffectBox, Instant, StageData, StageEffect, StageResponse, StageState,
};
use crate::{stagegraph::CallRef, CallId, Effect, ExternalEffect, Message, Name, StageRef, State};
use either::Either::{Left, Right};
use std::{
    collections::{BTreeMap, BinaryHeap, VecDeque},
    mem::{replace, take},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    task::{Context, Poll, Waker},
    time::Duration,
};
use tokio::{
    runtime::Handle,
    sync::oneshot::{Receiver, Sender},
};

/// Classification of why [`SimulationRunning::run_until_blocked`] has stopped.
#[derive(Debug, PartialEq)]
pub enum Blocked {
    /// All stages are suspended on [`Effect::Receive`].
    Idle,
    /// The simulation is waiting for a wakeup.
    Sleeping,
    /// All stages are suspended on either [`Effect::Receive`] or [`Effect::Send`].
    Deadlock(Vec<Name>),
    /// The given breakpoint was hit.
    Breakpoint(Name, Effect),
    /// The given stages are suspended on effects other than [`Effect::Receive`]
    /// while none are suspended on [`Effect::Send`].
    Busy(Vec<Name>),
}

impl Blocked {
    /// Assert that the blocking reason is `Idle`.
    pub fn assert_idle(&self) {
        match self {
            Blocked::Idle => {}
            _ => panic!("expected idle, got {:?}", self),
        }
    }

    /// Assert that the blocking reason is `Sleeping`.
    pub fn assert_sleeping(&self) {
        match self {
            Blocked::Sleeping => {}
            _ => panic!("expected sleeping, got {:?}", self),
        }
    }

    /// Assert that the blocking reason is `Deadlock` by at least the given stages.
    pub fn assert_deadlock(&self, names: impl IntoIterator<Item = impl AsRef<str>>) {
        let names = names
            .into_iter()
            .map(|n| Name::from(n.as_ref()))
            .collect::<Vec<_>>();
        match self {
            Blocked::Deadlock(deadlock) if deadlock.iter().all(|n| names.contains(n)) => {}
            _ => panic!("expected deadlock by {:?}, got {:?}", names, self),
        }
    }

    /// Assert that the blocking reason is `Busy` by at least the given stages.
    pub fn assert_busy(&self, names: impl IntoIterator<Item = impl AsRef<str>>) {
        let names = names
            .into_iter()
            .map(|n| Name::from(n.as_ref()))
            .collect::<Vec<_>>();
        match self {
            Blocked::Busy(busy) if names.iter().all(|n| busy.contains(n)) => {}
            _ => panic!("expected busy by {:?}, got {:?}", names, self),
        }
    }

    /// Assert that the blocking reason is `Breakpoint` by the given name.
    pub fn assert_breakpoint(self, name: impl AsRef<str>) -> Effect {
        match self {
            Blocked::Breakpoint(n, eff) if n.as_str() == name.as_ref() => eff,
            _ => panic!("expected breakpoint `{}`, got {:?}", name.as_ref(), self),
        }
    }
}

/// An entry for the sleeping stage heap.
///
/// NOTE: the `Ord` implementation is reversed, so that the heap is a min-heap.
/// The `wakeup` is secondarily ordered according to the address of the closure.
struct Sleeping {
    time: u64,
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
    #[allow(clippy::type_complexity)]
    transform: Box<
        dyn FnMut(
                Box<dyn ExternalEffect>,
            ) -> OverrideResult<Box<dyn ExternalEffect>, Box<dyn ExternalEffect>>
            + Send
            + 'static,
    >,
}

pub enum OverrideResult<In, Out> {
    /// The effect was not handled and shall be passed to overrides installed later than this one.
    NoMatch(In),
    /// The effect was handled and the message shall be delivered to the stage as the result.
    Handled(Box<dyn Message>),
    /// The effect was replaced by this new effect that will be run instead.
    Replaced(Out),
}

/// A handle to a running [`SimulationBuilder`](crate::simulation::SimulationBuilder).
///
/// It allows fine-grained control over single-stepping the simulation and when each
/// stage effect is resumed (using [`Self::try_effect`] and [`Self::resume_effect`],
/// respectively). This means that any interleaving of computations can be exercised.
/// Where this is not needed, you use [`Self::run_until_blocked`] to automate the
/// sending and receiving of messages within the simulated processing network.
///
/// Note that all stages start out in the state of waiting to receive their first message,
/// so you need to use [`resume_receive`](Self::resume_receive) to get them running.
/// See also [`run_until_blocked`](Self::run_until_blocked) for how to achieve this.
pub struct SimulationRunning {
    stages: BTreeMap<Name, StageData>,
    inputs: Inputs,
    effect: EffectBox,
    clock: Arc<AtomicU64>,
    now: Arc<dyn Fn() -> Instant + Send + Sync>,
    runnable: VecDeque<(Name, StageResponse)>,
    sleeping: BinaryHeap<Sleeping>,
    responded: Vec<(Name, CallId)>,
    mailbox_size: usize,
    rt: Handle,
    overrides: Vec<OverrideExternalEffect>,
    breakpoints: Vec<(Name, Box<dyn Fn(&Effect) -> bool + Send + 'static>)>,
}

impl SimulationRunning {
    pub(super) fn new(
        stages: BTreeMap<Name, StageData>,
        inputs: Inputs,
        effect: EffectBox,
        clock: Arc<AtomicU64>,
        now: Arc<dyn Fn() -> Instant + Send + Sync>,
        mailbox_size: usize,
        rt: Handle,
    ) -> Self {
        Self {
            stages,
            inputs,
            effect,
            clock,
            now,
            runnable: VecDeque::new(),
            sleeping: BinaryHeap::new(),
            responded: Vec::new(),
            mailbox_size,
            rt,
            overrides: Vec::new(),
            breakpoints: Vec::new(),
        }
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
                    #[allow(clippy::expect_used)]
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
        (self.now)()
    }

    /// Advance the clock to the next wakeup time.
    ///
    /// Returns `true` if a wakeup was skipped, `false` if there are no more wakeups.
    pub fn skip_to_next_wakeup(&mut self) -> bool {
        let Some(Sleeping { time, .. }) = self.sleeping.peek() else {
            return false;
        };
        let target = *time;
        let now = self.clock.swap(target, Ordering::Relaxed);
        assert!(now < target, "clock is ahead of next wakeup");
        while matches!(self.sleeping.peek(), Some(Sleeping { time, .. }) if *time == target) {
            let Sleeping { wakeup, .. } = self.sleeping.pop().expect("peeked, so must exist");
            wakeup(self);
        }
        true
    }

    fn schedule_wakeup(
        &mut self,
        nanos: u64,
        wakeup: impl FnOnce(&mut SimulationRunning) + Send + 'static,
    ) {
        assert!(nanos > 0, "cannot schedule wakeup with zero delay");
        let now = self.clock.load(Ordering::Relaxed);
        let time = now.checked_add(nanos).expect("clock wrapped around");
        self.sleeping.push(Sleeping {
            time,
            wakeup: Box::new(wakeup),
        });
    }

    /// Place messages in the given stage’s mailbox, but don’t resume it.
    /// The next message will be consumed when resuming an [`Effect::Receive`]
    /// for this stage.
    ///
    /// Note that this method does not check if there is enough space in the
    /// mailbox, it will grow the mailbox beyond the `mailbox_size` limit.
    pub fn enqueue_msg<T: Message, St>(
        &mut self,
        sr: &StageRef<T, St>,
        msg: impl IntoIterator<Item = T>,
    ) {
        let data = self.stages.get_mut(&sr.name).unwrap();
        data.mailbox
            .extend(msg.into_iter().map(|m| Box::new(m) as Box<dyn Message>));
    }

    /// Retrieve the number of messages currently in the given stage’s mailbox.
    pub fn mailbox_len<Msg, St>(&self, sr: &StageRef<Msg, St>) -> usize {
        let data = self.stages.get(&sr.name).unwrap();
        data.mailbox.len()
    }

    /// Obtain a reference to the current state of the given stage.
    /// This only works while the stage is suspended on an [`Effect::Receive`]
    /// because otherwise the state is captured by the opaque `Future` returned
    /// from the state transition function.
    ///
    /// Returns `None` if the stage is not suspended on [`Effect::Receive`], panics if the
    /// state type is incorrect.
    pub fn get_state<Msg, St: State>(&self, sr: &StageRef<Msg, St>) -> Option<&St> {
        let data = self.stages.get(&sr.name).unwrap();
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
    /// effect other than send — the latter case is called “busy” for want of a better term).
    pub fn try_effect(&mut self) -> Result<Effect, Blocked> {
        let Some((name, response)) = self.runnable.pop_front() else {
            let reason = block_reason(self);
            tracing::info!("blocking for reason: {:?}", reason);
            return Err(reason);
        };
        tracing::info!(name=%name, "resuming stage");

        let data = self
            .stages
            .get_mut(&name)
            .expect("stage was runnable, so it must exist");
        let StageState::Running(pin) = &mut data.state else {
            panic!(
                "runnable stage `{name}` is not running but {:?}",
                data.state
            );
        };

        *self.effect.lock() = Some(Right(response));
        let result = pin.as_mut().poll(&mut Context::from_waker(Waker::noop()));

        let ret = if let Poll::Ready(result) = result {
            match result {
                Ok(state) => {
                    data.state = StageState::Idle(state);
                    data.waiting = Some(StageEffect::Receive);
                    Ok(Effect::Receive { at_stage: name })
                }
                Err(error) => {
                    data.state = StageState::Failed;
                    Ok(Effect::Failure {
                        at_stage: name,
                        error,
                    })
                }
            }
        } else {
            let stage_effect = match self.effect.lock().take() {
                Some(Left(effect)) => effect,
                Some(Right(response)) => {
                    panic!(
                        "found response {response:?} instead of effect when polling stage `{name}`"
                    )
                }
                None => {
                    panic!("stage `{name}` returned without awaiting any tracked effect")
                }
            };
            let (wait_effect, effect) = stage_effect.split(name.clone());
            data.waiting = Some(wait_effect);
            Ok(effect)
        };

        let names = take(&mut self.responded);
        let runnable = &mut self.runnable;
        let run = &mut |name, response| {
            runnable.push_back((name, response));
        };
        for (name, id) in names {
            let data = self
                .stages
                .get_mut(&name)
                .expect("stage was responded to, so it must exist");
            // just trying to resume as far as possible, so failure to resume is okay
            Self::resume_call_internal(data, run, id).ok();
        }

        ret
    }

    /// Try to deliver external messages to stages that are waiting for them.
    ///
    /// Returns `InputsResult::Delivered(names)` if any messages were delivered,
    /// or `InputsResult::Blocked(name)` if delivery is blocked because the given
    /// stage's mailbox is full.
    pub fn try_inputs(&mut self) -> InputsResult {
        let mut delivered = Vec::new();
        while let Some(name) = self.inputs.peek_name().cloned() {
            let data = self.stages.get_mut(&name).unwrap();
            let mut envelope = self.inputs.try_next().expect("peeked, so must exist");
            let msg = replace(&mut envelope.msg, Box::new(()));
            if let Err(msg) = Self::post_message(data, self.mailbox_size, msg) {
                envelope.msg = msg;
                self.inputs.put_back(envelope);
                if delivered.is_empty() {
                    return InputsResult::Blocked(name);
                } else {
                    break;
                }
            } else {
                delivered.push(name);
                envelope.tx.send(()).ok();
            }
        }
        InputsResult::Delivered(delivered)
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
                Blocked::Sleeping => assert!(self.skip_to_next_wakeup()),
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
        self.try_inputs();

        {
            let runnable = &mut self.runnable;
            let run = &mut |name, response| {
                runnable.push_back((name, response));
            };
            for data in self.stages.values_mut() {
                let Some(StageEffect::Receive) = &data.waiting else {
                    continue;
                };
                Self::resume_receive_internal(data, run).ok();
            }
        }

        loop {
            let effect = match self.try_effect() {
                Ok(effect) => effect,
                Err(blocked) => return blocked,
            };

            tracing::info!(runnable = ?self.runnable.iter().map(|r| r.0.as_str()).collect::<Vec<&str>>(), effect = ?effect, "run effect");

            for (name, predicate) in &self.breakpoints {
                if (predicate)(&effect) {
                    tracing::info!("breakpoint `{}` hit: {:?}", name, effect);
                    return Blocked::Breakpoint(name.clone(), effect);
                }
            }

            self.handle_effect(effect);
        }
    }

    /// Handle the given effect as it would be by [`Self::run_until_sleeping_or_blocked`].
    /// This will resume the affected stage(s), it may involve multiple resumptions.
    ///
    /// Inputs to this method can be obtained from [`Self::effect`], [`Self::try_effect`]
    /// or [`Blocked::assert_breakpoint`].
    pub fn handle_effect(&mut self, effect: Effect) {
        let runnable = &mut self.runnable;
        let run = &mut |name, response| {
            runnable.push_back((name, response));
        };

        match effect {
            Effect::Receive { at_stage: to } => {
                let data_to = self.stages.get_mut(&to).unwrap();
                let Ok(()) = Self::resume_receive_internal(data_to, run) else {
                    return;
                };
                // resuming receive has removed one message from the mailbox, so check for blocked senders
                let Some((from, msg)) = data_to.senders.pop_front() else {
                    return;
                };
                Self::post_message(data_to, self.mailbox_size, msg).expect("mailbox is not full");
                let data_from = self.stages.get_mut(&from).unwrap();
                let call = Self::resume_send_internal(data_from, run, to.clone())
                    .expect("call is always runnable");
                self.handle_call_continuation(from, to, call);
            }
            Effect::Send {
                from,
                to,
                msg,
                call: _,
            } => {
                let data_to = self.stages.get_mut(&to).unwrap();
                if let Err(msg) = Self::post_message(data_to, self.mailbox_size, msg) {
                    data_to.senders.push_back((from, msg));
                } else {
                    // `to` may not be suspended on receive, so failure to resume is okay
                    Self::resume_receive_internal(data_to, run).ok();
                    let data_from = self.stages.get_mut(&from).unwrap();
                    let call = Self::resume_send_internal(data_from, run, to.clone())
                        .expect("call is always runnable");
                    self.handle_call_continuation(from, to, call);
                }
            }
            Effect::Clock { at_stage } => {
                let data = self.stages.get_mut(&at_stage).unwrap();
                Self::resume_clock_internal(data, run, (self.now)())
                    .expect("clock effect is always runnable");
            }
            Effect::Wait { at_stage, duration } => {
                let delay = duration_to_nanos(duration);
                self.schedule_wakeup(delay, move |sim| {
                    let data = sim
                        .stages
                        .get_mut(&at_stage)
                        .expect("stage ref exists, so stage must exist");
                    Self::resume_wait_internal(
                        data,
                        &mut |name, response| {
                            sim.runnable.push_back((name, response));
                        },
                        (sim.now)(),
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
                let data = self.stages.get_mut(&at_stage).unwrap();
                let res = Self::resume_respond_internal(data, run, target, id)
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
                let result = result.unwrap_or_else(|| self.rt.block_on(effect.run()));
                let data = self.stages.get_mut(&at_stage).unwrap();
                Self::resume_external_internal(data, result, run)
                    .expect("external effect is always runnable");
            }
            Effect::Failure { at_stage, error } => {
                panic!("stage `{at_stage}` failed with {error:?}");
            }
        }
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
                StageState::Failed => {
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
    pub fn resume_receive<Msg, St>(&mut self, at_stage: &StageRef<Msg, St>) -> anyhow::Result<()> {
        let data = self
            .stages
            .get_mut(&at_stage.name)
            .expect("stage ref exists, so stage must exist");
        Self::resume_receive_internal(data, &mut |name, response| {
            self.runnable.push_back((name, response));
        })
    }

    fn resume_receive_internal(
        data: &mut StageData,
        run: &mut dyn FnMut(Name, StageResponse),
    ) -> anyhow::Result<()> {
        let waiting_for = data.waiting.as_ref().ok_or_else(|| {
            anyhow::anyhow!("stage `{}` was not waiting for any effect", data.name)
        })?;

        if !matches!(waiting_for, StageEffect::Receive) {
            anyhow::bail!(
                "stage `{}` was not waiting for a receive effect, but {:?}",
                data.name,
                waiting_for
            )
        }

        let msg = data
            .mailbox
            .pop_front()
            .ok_or_else(|| anyhow::anyhow!("mailbox is empty while resuming receive"))?;

        // it is important that all validations (i.e. `?``) happen before this point
        data.waiting = None;

        let StageState::Idle(state) = replace(&mut data.state, StageState::Failed) else {
            panic!(
                "stage {} must have been Idle, was {:?}",
                data.name, data.state
            );
        };
        data.state = StageState::Running((data.transition)(state, msg));

        run(data.name.clone(), StageResponse::Unit);
        Ok(())
    }

    /// Resume an [`Effect::Send`].
    pub fn resume_send<Msg1, Msg2: Message, St1, St2>(
        &mut self,
        from: &StageRef<Msg1, St1>,
        to: &StageRef<Msg2, St2>,
        msg: Msg2,
    ) -> anyhow::Result<()> {
        let data = self
            .stages
            .get_mut(&to.name)
            .expect("stage ref exists, so stage must exist");
        if Self::post_message(data, self.mailbox_size, Box::new(msg)).is_err() {
            anyhow::bail!("mailbox is full while resuming send");
        }

        let data = self
            .stages
            .get_mut(&from.name)
            .expect("stage ref exists, so stage must exist");
        let call = Self::resume_send_internal(
            data,
            &mut |name, response| {
                self.runnable.push_back((name, response));
            },
            to.name(),
        )?;

        self.handle_call_continuation(from.name(), to.name(), call);
        Ok(())
    }

    fn handle_call_continuation(
        &mut self,
        from: Name,
        to: Name,
        call: Option<(Duration, Receiver<Box<dyn Message + 'static>>, CallId)>,
    ) {
        if let Some((timeout, recv, id)) = call {
            let deadline = (self.now)()
                .checked_add(timeout)
                .expect("timeout too large");
            self.stages
                .get_mut(&from)
                .expect("stage ref exists, so stage must exist")
                .waiting = Some(StageEffect::Call(to, deadline, (), recv, id));
            self.schedule_wakeup(duration_to_nanos(timeout), move |sim| {
                let data = sim
                    .stages
                    .get_mut(&from)
                    .expect("stage ref exists, so stage must exist");
                Self::resume_call_internal(
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

    fn post_message(
        data: &mut StageData,
        mailbox_size: usize,
        msg: Box<dyn Message>,
    ) -> Result<(), Box<dyn Message>> {
        if data.mailbox.len() >= mailbox_size {
            return Err(msg);
        }
        data.mailbox.push_back(msg);
        Ok(())
    }

    fn resume_send_internal(
        data: &mut StageData,
        run: &mut dyn FnMut(Name, StageResponse),
        to: Name,
    ) -> anyhow::Result<Option<(Duration, Receiver<Box<dyn Message>>, CallId)>> {
        let waiting_for = data.waiting.as_ref().ok_or_else(|| {
            anyhow::anyhow!("stage `{}` was not waiting for any effect", data.name)
        })?;

        if !matches!(waiting_for, StageEffect::Send(name, _msg, _call) if name == &to) {
            anyhow::bail!(
                "stage `{}` was not waiting for a send effect to `{}`, but {:?}",
                data.name,
                to,
                waiting_for
            )
        }

        // it is important that all validations (i.e. `?``) happen before this point
        let Some(StageEffect::Send(_to, _msg, call)) = data.waiting.take() else {
            panic!("match is guaranteed by the validation above");
        };

        if call.is_none() {
            run(data.name.clone(), StageResponse::Unit);
        }
        Ok(call)
    }

    /// Resume an [`Effect::Clock`].
    pub fn resume_clock<Msg, St>(
        &mut self,
        at_stage: &StageRef<Msg, St>,
        time: Instant,
    ) -> anyhow::Result<()> {
        let data = self
            .stages
            .get_mut(&at_stage.name)
            .expect("stage ref exists, so stage must exist");
        Self::resume_clock_internal(
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
    pub fn resume_wait<Msg, St>(
        &mut self,
        at_stage: &StageRef<Msg, St>,
        time: Instant,
    ) -> anyhow::Result<()> {
        let data = self
            .stages
            .get_mut(&at_stage.name)
            .expect("stage ref exists, so stage must exist");
        Self::resume_wait_internal(
            data,
            &mut |name, response| {
                self.runnable.push_back((name, response));
            },
            time,
        )
    }

    fn resume_wait_internal(
        data: &mut StageData,
        run: &mut dyn FnMut(Name, StageResponse),
        time: Instant,
    ) -> anyhow::Result<()> {
        let waiting_for = data.waiting.as_ref().ok_or_else(|| {
            anyhow::anyhow!("stage `{}` was not waiting for any effect", data.name)
        })?;

        if !matches!(waiting_for, StageEffect::Wait(_duration)) {
            anyhow::bail!(
                "stage `{}` was not waiting for a wait effect, but {:?}",
                data.name,
                waiting_for
            )
        }

        // it is important that all validations (i.e. `?``) happen before this point
        data.waiting = None;

        run(data.name.clone(), StageResponse::WaitResponse(time));
        Ok(())
    }

    /// Resume an [`Effect::Call`].
    ///
    /// If `msg` is `None`, the call has timed out.
    pub fn resume_call<Msg, St, Resp: Message>(
        &mut self,
        at_stage: &StageRef<Msg, St>,
        call: &CallRef<Resp>,
    ) -> anyhow::Result<()> {
        let data = self
            .stages
            .get_mut(&at_stage.name)
            .expect("stage ref exists, so stage must exist");
        Self::resume_call_internal(
            data,
            &mut |name, response| {
                self.runnable.push_back((name, response));
            },
            call.id,
        )
    }

    fn resume_call_internal(
        data: &mut StageData,
        run: &mut dyn FnMut(Name, StageResponse),
        id: CallId,
    ) -> anyhow::Result<()> {
        let waiting_for = data.waiting.as_ref().ok_or_else(|| {
            anyhow::anyhow!("stage `{}` was not waiting for any effect", data.name)
        })?;

        if !matches!(waiting_for, StageEffect::Call(_,_,_,_,id2) if id2 == &id) {
            anyhow::bail!(
                "stage `{}` was not waiting for a call effect, but {:?}",
                data.name,
                waiting_for
            )
        }

        // it is important that all validations (i.e. `?``) happen before this point
        let Some(StageEffect::Call(_name, _timeout, _msg, mut recv, _id)) = data.waiting.take()
        else {
            panic!("match is guaranteed by the validation above");
        };

        let msg = recv
            .try_recv()
            .ok()
            .map(StageResponse::CallResponse)
            .unwrap_or(StageResponse::CallTimeout);

        run(data.name.clone(), msg);
        Ok(())
    }

    /// Resume an [`Effect::Respond`].
    pub fn resume_respond<Msg, St, Resp: Message>(
        &mut self,
        at_stage: &StageRef<Msg, St>,
        cr: &CallRef<Resp>,
        msg: Resp,
    ) -> anyhow::Result<()> {
        let data = self
            .stages
            .get_mut(&at_stage.name)
            .expect("stage ref exists, so stage must exist");
        let res = Self::resume_respond_internal(
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
        msg: Box<dyn Message>,
        (target, id, deadline, sender): (Name, CallId, Instant, Sender<Box<dyn Message>>),
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

    fn resume_respond_internal(
        data: &mut StageData,
        run: &mut dyn FnMut(Name, StageResponse),
        target: Name,
        id: CallId,
    ) -> anyhow::Result<(Name, CallId, Instant, Sender<Box<dyn Message>>)> {
        let waiting_for = data.waiting.as_ref().ok_or_else(|| {
            anyhow::anyhow!("stage `{}` was not waiting for any effect", data.name)
        })?;

        if !matches!(waiting_for, StageEffect::Respond(name, id2, _deadline, _sender, _msg) if id2 == &id && name == &target)
        {
            anyhow::bail!(
                "stage `{}` was not waiting for a respond effect with id {id:?}, but {:?}",
                data.name,
                waiting_for
            )
        }

        // it is important that all validations (i.e. `?``) happen before this point
        let Some(StageEffect::Respond(target, _id2, deadline, sender, _msg)) = data.waiting.take()
        else {
            panic!("match is guaranteed by the validation above");
        };

        run(data.name.clone(), StageResponse::Unit);
        Ok((target, id, deadline, sender))
    }

    /// Resume an [`Effect::External`].
    pub fn resume_external<Msg, St>(
        &mut self,
        at_stage: &StageRef<Msg, St>,
        result: Box<dyn Message>,
    ) -> anyhow::Result<()> {
        let data = self
            .stages
            .get_mut(&at_stage.name)
            .expect("stage ref exists, so stage must exist");
        Self::resume_external_internal(data, result, &mut |name, response| {
            self.runnable.push_back((name, response));
        })
    }

    fn resume_external_internal(
        data: &mut StageData,
        result: Box<dyn Message>,
        run: &mut dyn FnMut(Name, StageResponse),
    ) -> anyhow::Result<()> {
        let waiting_for = data.waiting.as_ref().ok_or_else(|| {
            anyhow::anyhow!("stage `{}` was not waiting for any effect", data.name)
        })?;

        if !matches!(waiting_for, StageEffect::External(_)) {
            anyhow::bail!(
                "stage `{}` was not waiting for an external effect",
                data.name
            )
        }

        // it is important that all validations (i.e. `?``) happen before this point
        data.waiting = None;

        run(data.name.clone(), StageResponse::ExternalResponse(result));
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum InputsResult {
    Delivered(Vec<Name>),
    Blocked(Name),
}

fn duration_to_nanos(duration: std::time::Duration) -> u64 {
    u64::try_from(duration.as_nanos())
        .expect("duration too large")
        .max(1)
}

fn block_reason(sim: &SimulationRunning) -> Blocked {
    debug_assert!(sim.runnable.is_empty(), "runnable must be empty");
    if sim
        .stages
        .values()
        .filter_map(|d| d.waiting.as_ref())
        .all(|v| matches!(v, StageEffect::Receive))
    {
        if sim.sleeping.is_empty() {
            return Blocked::Idle;
        }
        return Blocked::Sleeping;
    }
    let mut send = Vec::new();
    let mut busy = Vec::new();
    let mut sleep = Vec::new();
    for (k, v) in sim
        .stages
        .iter()
        .filter_map(|(k, d)| d.waiting.as_ref().map(|w| (k, w)))
    {
        match v {
            StageEffect::Send(..) => send.push(k.clone()),
            StageEffect::Receive => {}
            StageEffect::Wait(..) => sleep.push(k.clone()),
            _ => busy.push(k.clone()),
        }
    }

    if !sleep.is_empty() {
        assert!(!sim.sleeping.is_empty()); // must be in there because nothing is runnable (yet)
        Blocked::Sleeping
    } else if !busy.is_empty() {
        Blocked::Busy(busy)
    } else if !send.is_empty() {
        Blocked::Deadlock(send)
    } else {
        Blocked::Idle
    }
}

#[test]
fn simulation_invariants() {
    use crate::{stagegraph::CallRef, StageGraph};

    tracing_subscriber::fmt()
        .with_test_writer()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init()
        .ok();

    let mut network = super::SimulationBuilder::default();
    let stage = network.stage("stage", async |_state, _msg: Option<CallRef<()>>, eff| {
        eff.send(&eff.me(), None).await;
        eff.clock().await;
        eff.wait(std::time::Duration::from_secs(1)).await;
        eff.call(&eff.me(), std::time::Duration::from_secs(1), Some)
            .await;
        Ok(true)
    });

    let stage = network.wire_up(stage, false);
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let mut sim = network.run(rt.handle().clone());

    #[allow(clippy::type_complexity)]
    let ops: [(
        Box<dyn Fn(&Effect) -> Option<CallId>>,
        Box<
            dyn Fn(
                &mut SimulationRunning,
                &StageRef<Option<CallRef<()>>, bool>,
                CallId,
            ) -> anyhow::Result<()>,
        >,
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
            Box::new(|sim, stage, _id| sim.resume_send(stage, stage, None)),
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
                    msg.cast_ref::<Option<CallRef<()>>>()
                        .expect("internal message type error")
                        .as_ref()
                        .unwrap()
                        .id,
                ),
                _ => None,
            }),
            // resume_call works because resume_send from the second item has already been called
            Box::new(|sim, stage, id| {
                let data = sim.stages.get_mut(&stage.name).unwrap();
                SimulationRunning::resume_call_internal(
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
    sim.enqueue_msg(&stage, [None]);
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
                op(&mut sim, &stage, CallId::from_u64(0)).unwrap_err();
                sim.invariants();
            }
        }
        for (pred, op, name) in &ops {
            if let Some(id) = pred(&effect) {
                tracing::info!("op `{}` should work", name);
                op(&mut sim, &stage, id).unwrap();
                sim.invariants();
            }
        }
    }
    tracing::info!("final invariants");
    sim.effect().assert_receive(&stage);
    let state = sim.get_state(&stage).unwrap();
    assert!(state);
}
