use super::{Effect, EffectBox, Instant, StageData, StageEffect, StageResponse, StageState};
use crate::{cast_state, Message, Name, StageRef, State};
use either::Either::{Left, Right};
use std::{
    collections::{HashMap, VecDeque},
    mem::replace,
    sync::{atomic::AtomicU64, Arc},
    task::{Context, Poll, Waker},
};

/// Classification of why [`SimulationRunning::run_until_blocked`] has stopped.
#[derive(Debug, PartialEq)]
pub enum Blocked {
    /// All stages are suspended on [`Effect::Receive`].
    Idle,
    /// All stages are suspended on either [`Effect::Receive`] or [`Effect::Send`].
    Deadlock(Vec<Name>),
    /// The given stage interrupted the simulation.
    Interrupted(Name),
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

    /// Assert that the blocking reason is `Interrupted` by the given stage.
    pub fn assert_interrupted(&self, name: impl AsRef<str>) {
        match self {
            Blocked::Interrupted(n) if n.as_str() == name.as_ref() => {}
            _ => panic!(
                "expected interrupted by `{}`, got {:?}",
                name.as_ref(),
                self
            ),
        }
    }

    /// Assert that the blocking reason is `Busy` by the given stages.
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
    stages: HashMap<Name, StageData>,
    effect: EffectBox,
    clock: Arc<AtomicU64>,
    runnable: VecDeque<(Name, StageResponse<Box<dyn Message>>)>,
    waiting: HashMap<Name, StageEffect<()>>,
    wait_send: HashMap<Name, VecDeque<(Name, Box<dyn Message>)>>,
    mailbox_size: usize,
}

impl SimulationRunning {
    pub(super) fn new(
        stages: HashMap<Name, StageData>,
        effect: EffectBox,
        clock: Arc<AtomicU64>,
        mailbox_size: usize,
    ) -> Self {
        // all stages start out suspended on Receive
        let waiting = stages
            .keys()
            .map(|k| (k.clone(), StageEffect::Receive))
            .collect();

        Self {
            stages,
            effect,
            clock,
            runnable: VecDeque::new(),
            waiting,
            wait_send: HashMap::new(),
            mailbox_size,
        }
    }

    /// Place messages in the given stage’s mailbox, but don’t resume it.
    /// The next message will be consumed when resuming an [`Effect::Receive`]
    /// for this stage.
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
    pub fn get_state<Msg, St: State>(&self, sr: &StageRef<Msg, St>) -> Option<&St> {
        let data = self.stages.get(&sr.name).unwrap();
        match &data.state {
            StageState::Idle(state) => Some(cast_state(&**state).unwrap()),
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
            let reason = block_reason(&self.waiting);
            tracing::info!("blocking for reason: {:?}", reason);
            return Err(reason);
        };
        tracing::info!("resuming stage: {}", name);

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

        if let Poll::Ready(result) = result {
            match result {
                Ok(state) => {
                    data.state = StageState::Idle(state);
                    self.waiting.insert(name.clone(), StageEffect::Receive);
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
            let (wait_effect, effect) = stage_effect.to_effect(name.clone());
            self.waiting.insert(name, wait_effect);
            Ok(effect)
        }
    }

    /// Keep on performing steps using [`Self::try_effect`] while possible and automatically
    /// resume send and receive effects based on availability of space or messages in the
    /// mailbox in question.
    pub fn run_until_blocked(&mut self) -> Blocked {
        let may_resume = self
            .waiting
            .iter()
            .filter(|(_, v)| matches!(v, StageEffect::Receive))
            .map(|(n, _)| n.clone())
            .collect::<Vec<_>>();
        for stage in may_resume {
            // it is okay if this fails, the mailbox may be empty
            let _ = self.resume_receive_internal(stage);
        }

        loop {
            let effect = match self.try_effect() {
                Ok(effect) => effect,
                Err(blocked) => return blocked,
            };
            tracing::info!(run = ?self.runnable, wait = ?self.waiting, "effect {:?}", effect);
            match effect {
                Effect::Receive { at_stage } => {
                    let data = self.stages.get_mut(&at_stage).unwrap();
                    if data.mailbox.is_empty() {
                        continue;
                    }
                    self.resume_receive_internal(at_stage.clone())
                        .expect("mailbox is not empty, so resume_receive must succeed");
                    let data = self.stages.get_mut(&at_stage).unwrap();
                    if data.mailbox.len() < self.mailbox_size {
                        if let Some((name, msg)) = self
                            .wait_send
                            .entry(at_stage.clone())
                            .or_default()
                            .pop_front()
                        {
                            self.resume_send_internal(name, at_stage, msg)
                                .expect("mailbox has space, so resume_send must succeed");
                        }
                    }
                }
                Effect::Send { from, to, msg } => {
                    let data = self.stages.get_mut(&to).unwrap();
                    if data.mailbox.len() < self.mailbox_size {
                        let empty = data.mailbox.is_empty();
                        self.resume_send_internal(from, to.clone(), msg)
                            .expect("mailbox has space, so resume_send must succeed");
                        if empty {
                            // try to resume (could fail because stage not currently waiting for Receive)
                            let _ = self.resume_receive_internal(to);
                        }
                    } else {
                        self.wait_send.entry(to).or_default().push_back((from, msg));
                    }
                }
                Effect::Interrupt { at_stage } => return Blocked::Interrupted(at_stage),
                _ => panic!("unexpected effect {effect:?}"),
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
            let waiting = self.waiting.get(name);
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
        self.resume_receive_internal(at_stage.name())
    }

    fn resume_receive_internal(&mut self, at_stage: Name) -> anyhow::Result<()> {
        let waiting_for = self.waiting.get(&at_stage).ok_or_else(|| {
            anyhow::anyhow!("stage `{}` was not waiting for any effect", at_stage)
        })?;

        if !matches!(waiting_for, StageEffect::Receive) {
            anyhow::bail!(
                "stage `{}` was not waiting for a receive effect, but {:?}",
                at_stage,
                waiting_for
            )
        }

        let data = self
            .stages
            .get_mut(&at_stage)
            .expect("stage was waiting, so it must exist");

        let msg = data
            .mailbox
            .pop_front()
            .ok_or_else(|| anyhow::anyhow!("mailbox is empty while resuming receive"))?;

        // it is important that all validations (i.e. `?``) happen before this point
        self.waiting.remove(&at_stage);

        let StageState::Idle(state) = replace(&mut data.state, StageState::Failed) else {
            panic!(
                "stage {} must have been Idle, was {:?}",
                at_stage, data.state
            );
        };
        data.state = StageState::Running((data.transition)(state, msg));

        self.runnable.push_back((at_stage, StageResponse::Unit));
        Ok(())
    }

    /// Resume an [`Effect::Send`].
    pub fn resume_send<Msg1, Msg2: Message, St1, St2>(
        &mut self,
        from: &StageRef<Msg1, St1>,
        to: &StageRef<Msg2, St2>,
        msg: Msg2,
    ) -> anyhow::Result<()> {
        self.resume_send_internal(from.name(), to.name(), Box::new(msg))
    }

    fn resume_send_internal(
        &mut self,
        from: Name,
        to: Name,
        msg: Box<dyn Message>,
    ) -> anyhow::Result<()> {
        let waiting_for = self
            .waiting
            .get(&from)
            .ok_or_else(|| anyhow::anyhow!("stage `{}` was not waiting for any effect", from))?;

        if !matches!(waiting_for, StageEffect::Send(name, _msg) if name == &to) {
            anyhow::bail!(
                "stage `{}` was not waiting for a send effect to `{}`, but {:?}",
                from,
                to,
                waiting_for
            )
        }

        let data = self
            .stages
            .get_mut(&to)
            .expect("stage was target of send, so it must exist");
        if data.mailbox.len() >= self.mailbox_size {
            anyhow::bail!("mailbox is full while resuming send");
        }

        // it is important that all validations (i.e. `?``) happen before this point
        self.waiting.remove(&from);

        data.mailbox.push_back(msg);

        self.runnable.push_back((from, StageResponse::Unit));
        Ok(())
    }

    /// Resume an [`Effect::Clock`].
    pub fn resume_clock<Msg, St>(
        &mut self,
        at_stage: &StageRef<Msg, St>,
        time: Instant,
    ) -> anyhow::Result<()> {
        self.resume_clock_internal(at_stage.name(), time)
    }

    fn resume_clock_internal(&mut self, at_stage: Name, time: Instant) -> anyhow::Result<()> {
        let waiting_for = self.waiting.get(&at_stage).ok_or_else(|| {
            anyhow::anyhow!("stage `{}` was not waiting for any effect", at_stage)
        })?;

        if !matches!(waiting_for, StageEffect::Clock) {
            anyhow::bail!(
                "stage `{}` was not waiting for a clock effect, but {:?}",
                at_stage,
                waiting_for
            )
        }

        // it is important that all validations (i.e. `?``) happen before this point
        self.waiting.remove(&at_stage);

        self.runnable
            .push_back((at_stage, StageResponse::ClockResponse(time)));
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
        self.resume_wait_internal(at_stage.name(), time)
    }

    fn resume_wait_internal(&mut self, at_stage: Name, time: Instant) -> anyhow::Result<()> {
        let waiting_for = self.waiting.get(&at_stage).ok_or_else(|| {
            anyhow::anyhow!("stage `{}` was not waiting for any effect", at_stage)
        })?;

        if !matches!(waiting_for, StageEffect::Wait(_duration)) {
            anyhow::bail!(
                "stage `{}` was not waiting for a wait effect, but {:?}",
                at_stage,
                waiting_for
            )
        }

        // it is important that all validations (i.e. `?``) happen before this point
        self.waiting.remove(&at_stage);

        self.runnable
            .push_back((at_stage, StageResponse::WaitResponse(time)));
        Ok(())
    }

    /// Resume an [`Effect::Call`].
    ///
    /// If `msg` is `None`, the call has timed out.
    pub fn resume_call<Msg, St>(
        &mut self,
        at_stage: &StageRef<Msg, St>,
        msg: Option<Box<dyn Message>>,
    ) -> anyhow::Result<()> {
        self.resume_call_internal(at_stage.name(), msg)
    }

    fn resume_call_internal(
        &mut self,
        at_stage: Name,
        msg: Option<Box<dyn Message>>,
    ) -> anyhow::Result<()> {
        let waiting_for = self.waiting.get(&at_stage).ok_or_else(|| {
            anyhow::anyhow!("stage `{}` was not waiting for any effect", at_stage)
        })?;

        if !matches!(waiting_for, StageEffect::Call(..)) {
            anyhow::bail!(
                "stage `{}` was not waiting for a call effect, but {:?}",
                at_stage,
                waiting_for
            )
        }

        // it is important that all validations (i.e. `?``) happen before this point
        self.waiting.remove(&at_stage);

        self.runnable.push_back((
            at_stage,
            msg.map(StageResponse::CallResponse)
                .unwrap_or(StageResponse::CallTimeout),
        ));
        Ok(())
    }

    /// Resume an [`Effect::Interrupt`].
    pub fn resume_interrupt<Msg, St>(
        &mut self,
        at_stage: &StageRef<Msg, St>,
    ) -> anyhow::Result<()> {
        self.resume_interrupt_internal(at_stage.name())
    }

    fn resume_interrupt_internal(&mut self, at_stage: Name) -> anyhow::Result<()> {
        let waiting_for = self.waiting.get(&at_stage).ok_or_else(|| {
            anyhow::anyhow!("stage `{}` was not waiting for any effect", at_stage)
        })?;

        if !matches!(waiting_for, StageEffect::Interrupt) {
            anyhow::bail!(
                "stage `{}` was not waiting for an interrupt effect",
                at_stage
            )
        }

        // it is important that all validations (i.e. `?``) happen before this point
        self.waiting.remove(&at_stage);

        self.runnable.push_back((at_stage, StageResponse::Unit));
        Ok(())
    }
}

fn block_reason(waiting: &HashMap<Name, StageEffect<()>>) -> Blocked {
    if waiting.values().all(|v| matches!(v, StageEffect::Receive)) {
        return Blocked::Idle;
    }
    let mut names = Vec::new();
    let mut busy = Vec::new();
    for (k, v) in waiting {
        match v {
            StageEffect::Send(..) => names.push(k.clone()),
            StageEffect::Receive => {}
            _ => busy.push(k.clone()),
        }
    }
    if busy.is_empty() {
        Blocked::Deadlock(names)
    } else {
        Blocked::Busy(busy)
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
    let stage = network.stage(
        "stage",
        async |_state, _msg: Option<CallRef<()>>, eff| {
            eff.send(&eff.me(), None).await;
            eff.clock().await;
            eff.wait(std::time::Duration::from_secs(1)).await;
            eff.call(&eff.me(), std::time::Duration::from_secs(1), |cr| Some(cr))
                .await;
            eff.interrupt().await;
            Ok(true)
        },
        false,
    );

    let stage = network.wire_up(stage, |_| {});
    let mut sim = network.run();

    let ops: [(
        Box<dyn Fn(&Effect) -> bool>,
        Box<
            dyn Fn(
                &mut SimulationRunning,
                &StageRef<Option<CallRef<()>>, bool>,
            ) -> anyhow::Result<()>,
        >,
        &'static str,
    ); 6] = [
        (
            Box::new(|eff: &Effect| matches!(eff, Effect::Receive { .. })),
            Box::new(|sim, stage| sim.resume_receive(stage)),
            "resume_receive",
        ),
        (
            Box::new(|eff: &Effect| matches!(eff, Effect::Send { .. })),
            Box::new(|sim, stage| sim.resume_send(stage, stage, None)),
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
            Box::new(|sim, stage| sim.resume_call(stage, None)),
            "resume_call",
        ),
        (
            Box::new(|eff: &Effect| matches!(eff, Effect::Interrupt { .. })),
            Box::new(|sim, stage| sim.resume_interrupt(stage)),
            "resume_interrupt",
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
        tracing::info!("effect {:?}", effect);
        assert!(
            ops[idx].0(&effect),
            "effect {effect:?} should match predicate for `{idx}`"
        );
        for (pred, op, name) in &ops {
            if !pred(&effect) {
                tracing::info!("op `{}` should not work", name);
                op(&mut sim, &stage).unwrap_err();
                sim.invariants();
            }
        }
        for (pred, op, name) in &ops {
            if pred(&effect) {
                tracing::info!("op `{}` should work", name);
                op(&mut sim, &stage).unwrap();
                sim.invariants();
            }
        }
    }
    tracing::info!("final invariants");
    sim.effect().assert_receive(&stage);
    let state = sim.get_state(&stage).unwrap();
    assert!(state);
}
