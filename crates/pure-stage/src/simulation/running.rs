use super::{Effect, StageData, StageEffect, StageState};
use crate::{cast_state, Message, Name, StageRef, State};
use parking_lot::Mutex;
use std::{
    collections::{HashMap, VecDeque},
    mem::replace,
    sync::Arc,
    task::{Context, Poll, Waker},
};

pub(super) type EffectBox = Arc<Mutex<Option<StageEffect<Box<dyn Message>>>>>;

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
pub struct SimulationRunning {
    stages: HashMap<Name, StageData>,
    effect: EffectBox,
    runnable: VecDeque<Name>,
    waiting: HashMap<Name, StageEffect<()>>,
    wait_send: HashMap<Name, VecDeque<(Name, Box<dyn Message>)>>,
    mailbox_size: usize,
}

impl SimulationRunning {
    pub(super) fn new(
        stages: HashMap<Name, StageData>,
        effect: EffectBox,
        runnable: VecDeque<Name>,
        mailbox_size: usize,
    ) -> Self {
        Self {
            stages,
            effect,
            runnable,
            waiting: HashMap::new(),
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
        let Some(name) = self.runnable.pop_front() else {
            let reason = block_reason(&self.waiting);
            tracing::info!("blocking for reason: {:?}", reason);
            return Err(reason);
        };
        tracing::info!("resuming stage: {}", name);
        let data = self.stages.get_mut(&name).unwrap();
        match &mut data.state {
            StageState::Idle(_state) => {
                self.waiting.insert(name.clone(), StageEffect::Receive);
                Ok(Effect::Receive { at_stage: name })
            }
            StageState::Running(pin) => {
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
                    let Some(effect) = self.effect.lock().take() else {
                        panic!("stage {} was not waiting for any effect", name);
                    };
                    match effect {
                        StageEffect::Receive => {
                            panic!("receive effect cannot be caused explicitly")
                        }
                        StageEffect::Send(to, msg) => {
                            self.waiting
                                .insert(name.clone(), StageEffect::Send(to.clone(), ()));
                            Ok(Effect::Send {
                                from: name,
                                to,
                                msg,
                            })
                        }
                        StageEffect::Clock => {
                            self.waiting.insert(name.clone(), StageEffect::Clock);
                            Ok(Effect::Clock { at_stage: name })
                        }
                        StageEffect::Wait(duration) => {
                            self.waiting
                                .insert(name.clone(), StageEffect::Wait(duration));
                            Ok(Effect::Wait {
                                at_stage: name,
                                duration,
                            })
                        }
                        StageEffect::Interrupt => {
                            self.waiting.insert(name.clone(), StageEffect::Interrupt);
                            Ok(Effect::Interrupt { at_stage: name })
                        }
                    }
                }
            }
            StageState::Failed => panic!("failed stage found in running list"),
        }
    }

    /// Keep on performing steps using [`Self::try_effect`] while possible and automatically
    /// resume send and receive effects based on availability of space or messages in the
    /// mailbox in question.
    pub fn run_until_blocked(&mut self) -> Blocked {
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
                    self.resume_receive(at_stage.clone());
                    let data = self.stages.get_mut(&at_stage).unwrap();
                    if data.mailbox.len() < self.mailbox_size {
                        if let Some((name, msg)) = self
                            .wait_send
                            .entry(at_stage.clone())
                            .or_default()
                            .pop_front()
                        {
                            self.resume_effect(Effect::Send {
                                from: name,
                                to: at_stage,
                                msg,
                            })
                            .unwrap();
                        }
                    }
                }
                Effect::Send { from, to, msg } => {
                    let data = self.stages.get_mut(&to).unwrap();
                    if data.mailbox.len() < self.mailbox_size {
                        let empty = data.mailbox.is_empty();
                        data.mailbox.push_back(msg);
                        if empty {
                            // try to resume (unless that has already been done or isn’t necessary right now)
                            let _ = self.resume_effect(Effect::Receive {
                                at_stage: to.clone(),
                            });
                        }
                        tracing::info!("immediately resuming `{}` after send to `{}`", from, to);
                        self.waiting.remove(&from);
                        self.runnable.push_back(from);
                    } else {
                        self.wait_send.entry(to).or_default().push_back((from, msg));
                    }
                }
                Effect::Interrupt { at_stage } => return Blocked::Interrupted(at_stage),
                _ => panic!("unexpected effect {effect:?}"),
            }
        }
    }

    /// Resume an [`Effect::Receive`].
    ///
    /// This will panic if the stage was not suspended on such an effect or if there is no message
    /// available in the mailbox.
    pub fn resume_receive(&mut self, at_stage: impl AsRef<str>) {
        self.resume_effect(Effect::Receive {
            at_stage: at_stage.as_ref().into(),
        })
        .unwrap();
    }

    /// Resume an [`Effect::Send`].
    ///
    /// This will panic if the stage was not suspended on such an effect. No check is performed on
    /// mailbox capacity.
    pub fn resume_send<T: Message>(&mut self, from: impl AsRef<str>, to: impl AsRef<str>, msg: T) {
        self.resume_effect(Effect::Send {
            from: from.as_ref().into(),
            to: to.as_ref().into(),
            msg: Box::new(msg),
        })
        .unwrap();
    }

    /// Resume an [`Effect::Interrupt`].
    ///
    /// This will panic if the stage was not suspended on such an effect.
    pub fn resume_interrupt(&mut self, at_stage: impl AsRef<str>) {
        self.resume_effect(Effect::Interrupt {
            at_stage: at_stage.as_ref().into(),
        })
        .unwrap();
    }

    /// Resume the given effect that was previously returned from [`Self::effect`] or [`Self::try_effect`].
    pub fn resume_effect(&mut self, effect: Effect) -> anyhow::Result<()> {
        let at_name = effect.at_stage();
        let Some(waiting_for) = self.waiting.get(at_name) else {
            anyhow::bail!("stage `{}` was not waiting for {:?}", at_name, effect)
        };
        waiting_for.assert_matching(at_name, &effect)?;
        tracing::info!(
            "resuming effect {:?} at {} with {:?}",
            waiting_for,
            at_name,
            effect
        );
        self.waiting.remove(at_name);
        let data = self.stages.get_mut(at_name).unwrap();
        match effect {
            Effect::Receive { at_stage } => {
                // cannot move out of data.state, so replace temporarily with dummy value
                let state = match replace(&mut data.state, StageState::Failed) {
                    StageState::Idle(state) => state,
                    state => {
                        // it is essential to put the state back, so that erroring does not change the state
                        data.state = state;
                        anyhow::bail!("stage {} must have been Idle", at_stage)
                    }
                };
                let Some(msg) = data.mailbox.pop_front() else {
                    anyhow::bail!("mailbox is empty while resuming receive")
                };
                data.state = StageState::Running((data.transition)(state, msg));
                self.runnable.push_back(at_stage);
            }
            Effect::Send { from, to, msg } => {
                let data = self.stages.get_mut(&to).unwrap();
                if data.mailbox.len() >= self.mailbox_size {
                    anyhow::bail!("mailbox is full while resuming send");
                }
                data.mailbox.push_back(msg);
                self.runnable.push_back(from);
            }
            Effect::Clock { at_stage, .. } => {
                self.runnable.push_back(at_stage);
            }
            Effect::Wait { at_stage, .. } => {
                self.runnable.push_back(at_stage);
            }
            Effect::Interrupt { at_stage } => {
                self.runnable.push_back(at_stage);
            }
            Effect::Failure { .. } => panic!("failure effect cannot be resumed"),
        }
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
