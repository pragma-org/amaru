use crate::{Message, StageBuildRef, StageRef, State};
use std::future::Future;

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
///     async |(mut state, out), msg: u32| {
///         state += msg;
///         out.send(state).await?;
///         Ok((state, out))
///     },
///     (1u32, StageRef::<u32, ()>::noop()),
/// );
/// // this is a feature of the SimulationBuilder
/// let (output, mut rx) = network.output("output");
///
/// // phase 2: wire up stages by injecting targets into their state
/// let stage = network.wire_up(stage, |state| state.1 = output);
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
        F: FnMut(St, Msg) -> Fut + 'static + Send,
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
