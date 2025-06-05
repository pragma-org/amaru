#![allow(
    clippy::wildcard_enum_match_arm,
    clippy::unwrap_used,
    clippy::panic,
    clippy::expect_used
)]

use crate::{
    effect::{StageEffect, StageResponse},
    BoxFuture, Effects, Instant, Message, Name, Sender, StageBuildRef, StageRef, State,
};
use std::{
    any::Any,
    collections::{BTreeMap, VecDeque},
    future::{poll_fn, Future},
    marker::PhantomData,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    task::Poll,
    time::Duration,
};

pub use running::{Blocked, OverrideResult, SimulationRunning};

use either::Either;
use inputs::Inputs;
use parking_lot::Mutex;
use state::{InitStageData, InitStageState, StageData, StageState, Transition};
use tokio::runtime::Handle;

mod inputs;
mod running;
mod state;

pub(crate) type EffectBox =
    Arc<Mutex<Option<Either<StageEffect<Box<dyn Message>>, StageResponse>>>>;

pub(crate) fn airlock_effect<Out>(
    eb: &EffectBox,
    effect: StageEffect<Box<dyn Message>>,
    mut response: impl FnMut(Option<StageResponse>) -> Option<Out> + Send + 'static,
) -> BoxFuture<'static, Out> {
    let eb = eb.clone();
    let mut effect = Some(effect);
    Box::pin(poll_fn(move |_| {
        let mut eb = eb.lock();
        if let Some(effect) = effect.take() {
            match eb.take() {
                Some(Either::Left(x)) => panic!("effect already set: {:?}", x),
                // it is either Some(Right(Unit)) after Receive or None otherwise
                Some(Either::Right(StageResponse::Unit)) | None => {}
                Some(Either::Right(resp)) => {
                    panic!("effect airlock contains leftover response: {:?}", resp)
                }
            }
            *eb = Some(Either::Left(effect));
            Poll::Pending
        } else {
            let Some(out) = eb.take() else {
                return Poll::Pending;
            };
            let out = match out {
                Either::Left(x) => panic!("expected response, got effect: {:?}", x),
                Either::Right(x) => response(Some(x)),
            };
            out.map(Poll::Ready).unwrap_or(Poll::Pending)
        }
    }))
}

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
/// use pure_stage::{StageGraph, simulation::SimulationBuilder, OutputEffect, ExternalEffect};
///
/// let mut network = SimulationBuilder::default();
/// let stage = network.stage("basic", async |(mut state, out), msg: u32, eff| {
///     state += msg;
///     eff.send(&out, state).await;
///     Ok((state, out))
/// });
/// let (output, mut rx) = network.output("output", 10);
/// let stage = network.wire_up(stage, (1u32, output.without_state()));
///
/// let rt = tokio::runtime::Runtime::new().unwrap();
/// let mut running = network.run(rt.handle().clone());
///
/// // first check that the stages start out suspended on Receive
/// running.try_effect().unwrap_err().assert_idle();
///
/// // then insert some input and check reaction
/// running.enqueue_msg(&stage, [1]);
/// running.resume_receive(&stage).unwrap();
/// running.effect().assert_send(&stage, &output, 2u32);
/// running.resume_send(&stage, &output, 2u32).unwrap();
/// running.effect().assert_receive(&stage);
///
/// running.resume_receive(&output).unwrap();
/// let ext = running.effect().extract_external(&output, &OutputEffect::fake(output.name(), 2u32).0);
/// let result = rt.block_on(ext.run());
/// running.resume_external(&output, result).unwrap();
/// running.effect().assert_receive(&output);
///
/// assert_eq!(rx.drain().collect::<Vec<_>>(), vec![2]);
/// ```
pub struct SimulationBuilder {
    stages: BTreeMap<Name, InitStageData>,
    effect: EffectBox,
    clock: Arc<AtomicU64>,
    now: Arc<dyn Fn() -> Instant + Send + Sync>,
    mailbox_size: usize,
    inputs: Inputs,
}

impl SimulationBuilder {
    pub fn with_mailbox_size(mut self, size: usize) -> Self {
        self.mailbox_size = size;
        self
    }
}

impl Default for SimulationBuilder {
    fn default() -> Self {
        let clock_base = tokio::time::Instant::now();
        let clock = Arc::new(AtomicU64::new(0));
        let clock2 = clock.clone();
        let now = Arc::new(move || {
            Instant::from_tokio(clock_base + Duration::from_nanos(clock2.load(Ordering::Relaxed)))
        });

        Self {
            stages: Default::default(),
            effect: Default::default(),
            clock,
            now,
            mailbox_size: 10,
            inputs: Inputs::new(10),
        }
    }
}

impl super::StageGraph for SimulationBuilder {
    type Running = SimulationRunning;
    type RefAux<Msg, State> = ();

    fn stage<Msg: Message, St: State, F, Fut>(
        &mut self,
        name: impl AsRef<str>,
        mut f: F,
    ) -> StageBuildRef<Msg, St, Self::RefAux<Msg, St>>
    where
        F: FnMut(St, Msg, Effects<Msg, St>) -> Fut + 'static + Send,
        Fut: Future<Output = anyhow::Result<St>> + 'static + Send,
    {
        // THIS MUST MATCH THE TOKIO BUILDER
        let name = Name::from(&*format!("{}-{}", name.as_ref(), self.stages.len()));
        let me = StageRef {
            name: name.clone(),
            _ph: PhantomData,
        };
        let self_sender = self.inputs.sender(&me);
        let effects = Effects::new(me, self.effect.clone(), self.now.clone(), self_sender);
        let transition: Transition =
            Box::new(move |state: Box<dyn State>, msg: Box<dyn Message>| {
                let state = (state as Box<dyn Any>).downcast::<St>().unwrap();
                let msg = *msg.cast::<Msg>().expect("internal message type error");
                let state = f(*state, msg, effects.clone());
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
            #[allow(clippy::panic)]
            {
                // names are unique by construction
                panic!("stage {name} already exists with state {:?}", old.state);
            }
        }

        StageBuildRef {
            name,
            network: (),
            _ph: PhantomData,
        }
    }

    fn wire_up<Msg: Message, St: State>(
        &mut self,
        stage: crate::StageBuildRef<Msg, St, Self::RefAux<Msg, St>>,
        state: St,
    ) -> StageRef<Msg, St> {
        let StageBuildRef {
            name,
            network: (),
            _ph,
        } = stage;

        let data = self.stages.get_mut(&name).unwrap();
        data.state = InitStageState::Idle(Box::new(state));

        StageRef {
            name,
            _ph: PhantomData,
        }
    }

    fn sender<Msg: Message, St>(&mut self, stage: &StageRef<Msg, St>) -> Sender<Msg> {
        self.inputs.sender(stage)
    }

    fn run(self, rt: Handle) -> Self::Running {
        let Self {
            stages: s,
            effect,
            clock,
            now,
            mailbox_size,
            inputs,
        } = self;
        let mut stages = BTreeMap::new();
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
                name: name.clone(),
                mailbox,
                state,
                transition,
                waiting: Some(StageEffect::Receive),
                senders: VecDeque::new(),
            };
            stages.insert(name, data);
        }
        SimulationRunning::new(stages, inputs, effect, clock, now, mailbox_size, rt)
    }
}
