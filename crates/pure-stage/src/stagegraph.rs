use crate::{
    Effects, Instant, Message, Name, OutputEffect, Receiver, Sender, StageBuildRef, StageRef,
    State, Void,
};
use std::{
    fmt::Debug,
    future::Future,
    marker::PhantomData,
    sync::atomic::{AtomicU64, Ordering},
};
use tokio::{
    runtime::Handle,
    sync::{mpsc, oneshot},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CallId(u64);

impl CallId {
    pub(crate) fn new() -> Self {
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
    pub(crate) _ph: PhantomData<Resp>,
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
/// use pure_stage::{StageGraph, simulation::SimulationBuilder};
///
/// let mut network = SimulationBuilder::default();
///
/// // phase 1: create stages
/// let stage = network.stage("basic", async |(mut state, out), msg: u32, eff| {
///     state += msg;
///     eff.send(&out, state).await;
///     Ok((state, out))
/// });
/// // this is a feature of the SimulationBuilder
/// let (output, mut rx) = network.output("output", 10);
///
/// // phase 2: wire up stages by injecting targets into their state
/// let stage = network.wire_up(stage, (1u32, output.without_state()));
///
/// // finalize the network and run it (or make it controllable, in case of SimulationBuilder)
/// // (this needs a Tokio runtime for executing external effects)
/// let rt = tokio::runtime::Runtime::new().unwrap();
/// let mut running = network.run(rt.handle().clone());
/// ```
pub trait StageGraph {
    type Running;
    type RefAux<Msg, State>;

    /// Create a stage from an asynchronous transition function (state × message → state) and
    /// an initial state.
    ///
    /// _The provided name will be made unique within this `StageGraph` by appending a number!_
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
        state: St,
    ) -> StageRef<Msg, St>;

    /// Consume this network builder and start the network — the precise meaning of this
    /// depends on the `StageGraph` implementation used.
    ///
    /// For example [`TokioBuilder`](crate::tokio::TokioBuilder) will spawn each stage as
    /// a task while [`SimulationBuilder`](crate::simulation::SimulationBuilder) won’t
    /// run anything unless explicitly requested by a test procedure.
    fn run(self, rt: Handle) -> Self::Running;

    /// Obtain a handle for sending messages to the given stage from outside the network.
    fn sender<Msg: Message, St>(&mut self, stage: &StageRef<Msg, St>) -> Sender<Msg>;

    /// Utility function to create an output for the network.
    ///
    /// The returned [`Receiver`] can be used synchronously (e.g. in tests) or as an
    /// asynchronous [`Stream`](futures_util::Stream).
    fn output<Msg: Message + PartialEq>(
        &mut self,
        name: impl AsRef<str>,
        send_queue_size: usize,
    ) -> (StageRef<Msg, Void>, Receiver<Msg>) {
        let name = Name::from(name.as_ref());
        let (tx, rx) = mpsc::channel(send_queue_size);

        let output = self.stage(name, async |tx: mpsc::Sender<Msg>, msg: Msg, eff| {
            eff.external(OutputEffect::new(eff.me().name(), msg, tx.clone()))
                .await;
            Ok(tx)
        });
        let output = self.wire_up(output, tx);

        (output.without_state(), Receiver::new(rx))
    }
}
