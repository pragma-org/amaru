use crate::{
    cast_msg,
    effect::{StageEffect, StageResponse},
    simulation::{airlock_effect, EffectBox},
    BoxFuture, Instant, Message, Name, StageBuildRef, StageRef, State,
};
use std::{
    fmt::Debug,
    future::Future,
    marker::PhantomData,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::sync::oneshot;

pub struct Effects<M, S> {
    me: StageRef<M, S>,
    effect: EffectBox,
    now: Arc<dyn Fn() -> Instant + Send + Sync>,
}

impl<M, S> Clone for Effects<M, S> {
    fn clone(&self) -> Self {
        Self {
            me: self.me.clone(),
            effect: self.effect.clone(),
            now: self.now.clone(),
        }
    }
}

impl<M: Debug, S: Debug> Debug for Effects<M, S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Effects")
            .field("me", &self.me)
            .field("effect", &self.effect)
            .finish()
    }
}

impl<M: Message, S> Effects<M, S> {
    pub(crate) fn new(
        me: StageRef<M, S>,
        effect: EffectBox,
        now: Arc<dyn Fn() -> Instant + Send + Sync>,
    ) -> Self {
        Self { me, effect, now }
    }

    pub fn me(&self) -> StageRef<M, S> {
        self.me.clone()
    }

    pub fn send<Msg: Message, St>(
        &self,
        target: &StageRef<Msg, St>,
        msg: Msg,
    ) -> BoxFuture<'static, ()> {
        airlock_effect(
            &self.effect,
            StageEffect::Send(target.name(), Box::new(msg), None),
            |_eff| Some(()),
        )
    }

    pub fn interrupt(&self) -> BoxFuture<'static, ()> {
        airlock_effect(&self.effect, StageEffect::Interrupt, |_eff| Some(()))
    }

    pub fn clock(&self) -> BoxFuture<'static, Instant> {
        airlock_effect(&self.effect, StageEffect::Clock, |eff| match eff {
            Some(StageResponse::ClockResponse(instant)) => Some(instant),
            _ => None,
        })
    }

    pub fn wait(&self, duration: Duration) -> BoxFuture<'static, Instant> {
        airlock_effect(&self.effect, StageEffect::Wait(duration), |eff| match eff {
            Some(StageResponse::WaitResponse(instant)) => Some(instant),
            _ => None,
        })
    }

    pub fn call<Req: Message, Resp: Message, St>(
        &self,
        target: &StageRef<Req, St>,
        timeout: Duration,
        msg: impl FnOnce(CallRef<Resp>) -> Req + Send + 'static,
    ) -> BoxFuture<'static, Option<Resp>> {
        let (response, recv) = oneshot::channel();
        let now = (self.now)();
        let deadline = now.checked_add(timeout).expect("timeout too long");
        let target = target.name();
        let me = self.me.name();
        let id = CallId::new();

        let msg = Box::new(msg(CallRef {
            target: me,
            id,
            deadline,
            response,
            _ph: PhantomData,
        }));

        airlock_effect(
            &self.effect,
            StageEffect::Send(target, msg, Some((timeout, recv, id))),
            |eff| match eff {
                Some(StageResponse::CallResponse(resp)) => Some(Some(
                    cast_msg::<Resp>(resp).expect("internal messaging type error"),
                )),
                Some(StageResponse::CallTimeout) => Some(None),
                _ => None,
            },
        )
    }

    pub fn respond<Resp: Message>(&self, cr: CallRef<Resp>, resp: Resp) -> BoxFuture<'static, ()> {
        let CallRef {
            target,
            id,
            deadline,
            response,
            _ph,
        } = cr;
        airlock_effect(
            &self.effect,
            StageEffect::Respond(target, id, deadline, response, Box::new(resp)),
            |_eff| Some(()),
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CallId(u64);

impl CallId {
    fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        Self(COUNTER.fetch_add(1, Ordering::Relaxed))
    }

    #[cfg(test)]
    pub(crate) fn from_u64(u: u64) -> Self {
        Self(u)
    }
}

/// The response channel for a call effect.
///
/// In order to respond to the calling stage, use [`Effects::respond`].
pub struct CallRef<Resp: Message> {
    pub(crate) target: Name,
    pub(crate) id: CallId,
    pub(crate) deadline: Instant,
    pub(crate) response: oneshot::Sender<Box<dyn Message>>,
    _ph: PhantomData<Resp>,
}

impl<Resp: Message + PartialEq> PartialEq for CallRef<Resp> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl<Resp: Message> Debug for CallRef<Resp> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CallRef")
            .field("target", &self.target)
            .field("id", &self.id)
            .field("deadline", &self.deadline)
            .finish()
    }
}

impl<Resp: Message> CallRef<Resp> {
    /// Create a dummy that compares equal to its original but doesn’t actually work.
    ///
    /// This is useful within test procedures to retain a properly typed reference
    /// that can be used as argument to [`assert_respond`](crate::simulation::Effect::assert_respond)
    /// and [`resume_respond`](crate::simulation::SimulationRunning::resume_respond).
    pub fn dummy(&self) -> Self {
        Self {
            target: self.target.clone(),
            id: self.id,
            deadline: self.deadline,
            response: oneshot::channel().0,
            _ph: PhantomData,
        }
    }
}

/// A factory for processing network stages and their wiring.
///
/// Network construction proceeds in two phases:
/// 1. create stages, providing their initial states and transition functions, but using
///    [dummy stage references](StageRef::noop) for messaging targets
/// 2. wire up the stages by injecting the real [`StageRef`](StageRef) messaging targets
///
/// Sending to a `noop` target does nothing, it also doesn’t suspend the stage to create
/// an effect when using [`SimulationBuilder`](crate::simulation::SimulationBuilder).
/// If you forget to call [`wire_up`](StageGraph::wire_up) on
///
/// Example:
/// ```rust
/// use pure_stage::{StageGraph, simulation::SimulationBuilder, StageRef};
///
/// let mut network = SimulationBuilder::default();
///
/// // phase 1: create stages
/// let stage = network.stage(
///     "basic",
///     async |(mut state, out), msg: u32, eff| {
///         state += msg;
///         eff.send(&out, state).await;
///         Ok((state, out))
///     },
///     (1u32, StageRef::noop::<u32>()),
/// );
/// // this is a feature of the SimulationBuilder
/// let (output, mut rx) = network.output("output");
///
/// // phase 2: wire up stages by injecting targets into their state
/// let stage = network.wire_up(stage, |state| state.1 = output.without_state());
///
/// // finalize the network and run it (or make it controllable, in case of SimulationBuilder)
/// let mut running = network.run();
/// ```
pub trait StageGraph {
    type Running;
    type RefAux<Msg, State>;

    /// Create a stage from an asynchronous transition function (state × message → state) and
    /// an initial state.
    ///
    /// The provided name needs to be unique within this `StageGraph`.
    ///
    /// **IMPORTANT:** While the transition function is asynchronous, it cannot await asynchronous
    /// effects other than those constructed by this library.
    ///
    /// Typically, the transition function will be a function pointer, in which case the type
    /// for the initial state is fixed by the function signature. If you provide a closure, you
    /// may using `async |...| { ... }` if it doesn’t capture from the environment only; the
    /// reason is that otherwise the returned [`Future`] isn’t `'static` because it will reference
    /// the values captured by the function. If you need this, captured values need to be `Clone`
    /// and the pattern is
    ///
    /// ```no-compile
    /// move |state, msg| {
    ///     let captured = captured.clone();
    ///     async move {
    ///         // use `captured` here
    ///     }
    /// }
    /// ```
    fn stage<Msg: Message, St: State, F, Fut>(
        &mut self,
        name: impl AsRef<str>,
        f: F,
        state: St,
    ) -> StageBuildRef<Msg, St, Self::RefAux<Msg, St>>
    where
        F: FnMut(St, Msg, Effects<Msg, St>) -> Fut + 'static + Send,
        Fut: Future<Output = anyhow::Result<St>> + 'static + Send;

    /// Finalize the given stage.
    ///
    /// A mutable reference to the stage’s state is provided, mainly for the purpose of
    /// injecting real [`StageRef`] where initially only [`StageRef::noop`] was provided.
    ///
    /// It is good practice set all other state when creating the stage, and capturing
    /// via closures is discouraged because only the explicit state is accessible when
    /// using [`SimulationBuilder`](crate::simulation::SimulationBuilder).
    fn wire_up<Msg: Message, St: State>(
        &mut self,
        stage: StageBuildRef<Msg, St, Self::RefAux<Msg, St>>,
        f: impl FnOnce(&mut St),
    ) -> StageRef<Msg, St>;

    /// Consume this network builder and start the network — the precise meaning of this
    /// depends on the `StageGraph` implementation used.
    ///
    /// For example [`TokioBuilder`](crate::tokio::TokioBuilder) will spawn each stage as
    /// a task while [`SimulationBuilder`](crate::simulation::SimulationBuilder) won’t
    /// run anything unless explicitly requested by a test procedure.
    fn run(self) -> Self::Running;
}
