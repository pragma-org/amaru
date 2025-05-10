#![allow(clippy::wildcard_enum_match_arm, clippy::unwrap_used, clippy::panic)]

use crate::{cast_msg, BoxFuture, Message, Name, StageBuildRef, StageGraph, StageRef, State};
use std::{
    any::Any,
    collections::{HashMap, VecDeque},
    future::{poll_fn, Future},
    marker::PhantomData,
    sync::Arc,
    task::Poll,
};
use tokio::sync::mpsc::unbounded_channel;

pub use effect::Effect;
pub use interrupt::Interrupter;
pub use receiver::Receiver;
pub use running::{Blocked, SimulationRunning};

use effect::StageEffect;
use running::EffectBox;
use state::{InitStageData, InitStageState, StageData, StageState, Transition};

mod effect;
mod interrupt;
mod receiver;
mod running;
mod state;

/// A fully controllable and deterministic [`StageGraph`] for testing purposes.
///
/// Execution is controlled entirely via the [`SimulationRunning`] handle returned from
/// [`StageGraph::run`].
///
/// The general principle is that each stage is suspended whenever it needs new
/// input (even when there is a message available in the mailbox) or when it uses
/// any of the effects provided (like [`StageRef::send`] or [`Interrupter::interrupt`]).
/// Resuming the given effect will not run the stage, but it will make it runnable
/// again when performing the next simulation step.
///
/// Example:
/// ```rust
/// use pure_stage::{StageGraph, simulation::SimulationBuilder, StageRef};
///
/// let mut network = SimulationBuilder::default();
/// let stage = network.stage(
///     "basic",
///     async |(mut state, out), msg: u32| {
///         state += msg;
///         out.send(state).await?;
///         Ok((state, out))
///     },
///     (1u32, StageRef::<u32, ()>::noop()),
/// );
/// let (output, mut rx) = network.output("output");
/// let stage = network.wire_up(stage, |state| state.1 = output);
/// let mut running = network.run();
///
/// // first check that the stages start receiving
/// running.effect().assert_receive("output");
/// running.effect().assert_receive("basic");
/// running.try_effect().unwrap_err().assert_idle();
///
/// // then insert some input and check reaction
/// running.enqueue_msg(&stage, [1]);
/// running.resume_receive("basic");
/// running.effect().assert_send("basic", "output", 2u32);
/// running.resume_send("basic", "output", 2u32);
/// running.effect().assert_receive("basic");
///
/// running.resume_receive("output");
/// running.effect().assert_receive("output");
///
/// assert_eq!(rx.drain().collect::<Vec<_>>(), vec![2]);
/// ```
pub struct SimulationBuilder {
    stages: HashMap<Name, InitStageData>,
    runnable: VecDeque<Name>,
    effect: EffectBox,
    mailbox_size: usize,
}

impl SimulationBuilder {
    pub fn with_mailbox_size(mut self, size: usize) -> Self {
        self.mailbox_size = size;
        self
    }
}

impl Default for SimulationBuilder {
    fn default() -> Self {
        Self {
            stages: Default::default(),
            runnable: Default::default(),
            effect: Default::default(),
            mailbox_size: 10,
        }
    }
}

impl SimulationBuilder {
    /// Construct a stage that sends received messages to a [`Receiver`] that is also returned.
    ///
    /// Note that you can control the forwarding of these messages by delaying the resumption of
    /// the [`Effect::Receive`] if you run in single-step mode. Otherwise, it is also possible
    /// to have this stage interrupt the simulation when an incoming message fits a given predicate,
    /// see [`output_interrupt`](Self::output_interrupt).
    pub fn output<T: Message>(&mut self, name: impl AsRef<str>) -> (StageRef<T, ()>, Receiver<T>) {
        let (tx, rx) = unbounded_channel();
        let stage = self.stage(
            &name,
            move |_st, msg| {
                let tx = tx.clone();
                async move { tx.send(msg).map_err(|_| anyhow::anyhow!("channel closed")) }
            },
            (),
        );
        let StageRef { name, send, .. } = self.wire_up(stage, |_| {});
        (
            StageRef {
                name,
                send,
                _ph: PhantomData,
            },
            Receiver::new(rx),
        )
    }

    /// Construct a stage that sends received messages to a [`Receiver`] that is also returned.
    ///
    /// Note that you can control the forwarding of these messages by delaying the resumption of
    /// the [`Effect::Receive`] if you run in single-step mode. Otherwise, it is also possible
    /// to have this stage interrupt the simulation when an incoming message fits a given predicate.
    ///
    /// Example:
    /// ```rust
    /// # use pure_stage::{StageGraph, simulation::SimulationBuilder};
    /// let mut network = SimulationBuilder::default();
    /// let (output, mut rx) = network.output_interrupt("output", |msg| *msg == 2);
    /// let mut running = network.run();
    ///
    /// running.enqueue_msg(&output, [1, 2, 3]);
    ///
    /// running.run_until_blocked().assert_interrupted("output");
    /// assert_eq!(rx.drain().collect::<Vec<_>>(), vec![1]);
    ///
    /// running.resume_interrupt("output");
    /// assert_eq!(rx.drain().collect::<Vec<_>>(), vec![]);
    ///
    /// running.run_until_blocked().assert_idle();
    /// assert_eq!(rx.drain().collect::<Vec<_>>(), vec![2, 3]);
    /// ```
    pub fn output_interrupt<T: Message>(
        &mut self,
        name: impl AsRef<str>,
        interrupt: impl Fn(&T) -> bool + Send + 'static,
    ) -> (StageRef<T, ()>, Receiver<T>) {
        let (tx, rx) = unbounded_channel();
        let inter = self.interrupter();
        let stage = self.stage(
            &name,
            move |_st, msg| {
                let tx = tx.clone();
                let interrupt = interrupt(&msg).then(|| inter.clone());
                async move {
                    if let Some(interrupt) = interrupt {
                        interrupt.interrupt().await?;
                    }
                    tx.send(msg).map_err(|_| anyhow::anyhow!("channel closed"))
                }
            },
            (),
        );
        let StageRef { name, send, .. } = self.wire_up(stage, |_| {});
        (
            StageRef {
                name,
                send,
                _ph: PhantomData,
            },
            Receiver::new(rx),
        )
    }

    /// Construct a factory for interruption effects.
    ///
    /// This can be cloned and used in multiple stages to interrupt an ongoing simulation
    /// while using [`SimulationRunning::run_until_blocked`] when you don’t want the network to run
    /// until no further actions can be taken automatically.
    pub fn interrupter(&self) -> Interrupter {
        Interrupter::new(self.effect.clone())
    }
}

impl super::StageGraph for SimulationBuilder {
    type Running = SimulationRunning;
    type RefAux<Msg, State> = ();

    fn stage<Msg: Message, St: State, F, Fut>(
        &mut self,
        name: impl AsRef<str>,
        mut f: F,
        state: St,
    ) -> StageBuildRef<Msg, St, Self::RefAux<Msg, St>>
    where
        F: FnMut(St, Msg) -> Fut + 'static + Send,
        Fut: Future<Output = anyhow::Result<St>> + 'static + Send,
    {
        let name = Name::from(name.as_ref());
        let transition: Transition =
            Box::new(move |state: Box<dyn State>, msg: Box<dyn Message>| {
                let state = (state as Box<dyn Any>).downcast::<St>().unwrap();
                let msg = cast_msg::<Msg>(msg).unwrap();
                let state = f(*state, msg);
                Box::pin(async move { Ok(Box::new(state.await?) as Box<dyn State>) })
            });

        if let Some(old) = self.stages.insert(
            name.clone(),
            InitStageData {
                state: InitStageState::Uninitialized,
                mailbox: VecDeque::new(),
                transition,
            },
        ) {
            panic!("stage {name} already exists with state {:?}", old.state);
        }

        let effect = self.effect.clone();
        let target = name.clone();
        let send = Arc::new(move |msg: Msg| {
            let mut eff = Some(StageEffect::Send(
                target.clone(),
                Box::new(msg) as Box<dyn Message>,
            ));
            let effect = effect.clone();
            Box::pin(poll_fn(move |_| {
                if let Some(eff) = eff.take() {
                    let mut effect = effect.lock();
                    assert!(effect.is_none(), "effect already set");
                    *effect = Some(eff);
                    Poll::Pending
                } else {
                    match *effect.lock() {
                        None => Poll::Ready(Ok(())),
                        _ => Poll::Pending,
                    }
                }
            })) as BoxFuture<'static, anyhow::Result<()>>
        });

        StageBuildRef {
            name,
            state,
            send,
            network: (),
        }
    }

    fn wire_up<Msg: Message, St: State>(
        &mut self,
        stage: crate::StageBuildRef<Msg, St, Self::RefAux<Msg, St>>,
        f: impl FnOnce(&mut St),
    ) -> StageRef<Msg, St> {
        let StageBuildRef {
            name,
            mut state,
            network: (),
            send,
        } = stage;

        f(&mut state);
        let data = self.stages.get_mut(&name).unwrap();
        data.state = InitStageState::Idle(Box::new(state));

        self.runnable.push_back(name.clone());

        StageRef {
            name,
            send,
            _ph: PhantomData,
        }
    }

    fn run(self) -> Self::Running {
        let Self {
            stages: s,
            effect,
            runnable,
            mailbox_size,
        } = self;
        let mut stages = HashMap::new();
        for (
            name,
            InitStageData {
                mailbox,
                state,
                transition,
            },
        ) in s
        {
            let state = match state {
                InitStageState::Uninitialized => panic!("forgot to wire up stage `{name}`"),
                InitStageState::Idle(state) => StageState::Idle(state),
            };
            let data = StageData {
                mailbox,
                state,
                transition,
            };
            stages.insert(name.clone(), data);
        }
        SimulationRunning::new(stages, effect, runnable, mailbox_size)
    }
}
