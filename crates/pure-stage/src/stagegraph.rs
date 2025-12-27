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

use crate::stage_ref::StageStateRef;
use crate::{
    BoxFuture, Effects, Instant, Name, OutputEffect, Receiver, Resources, SendData, Sender,
    StageBuildRef, StageRef, types::MpscSender,
};
use std::{
    fmt::Debug,
    future::Future,
    marker::PhantomData,
    sync::atomic::{AtomicU64, Ordering},
};
use tokio::sync::{mpsc, oneshot};

/// A unique identifier for a call effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
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
#[derive(serde::Serialize, serde::Deserialize)]
pub struct CallRef<Resp: SendData> {
    pub(crate) target: Name,
    pub(crate) id: CallId,
    pub(crate) deadline: Instant,
    #[serde(skip, default = "dummy_response")]
    pub(crate) response: oneshot::Sender<Box<dyn SendData>>,
    #[serde(skip)]
    pub(crate) _ph: PhantomData<Resp>,
}

// FIXME: will need to inject deserialization context to reconstruct a channel
fn dummy_response() -> oneshot::Sender<Box<dyn SendData>> {
    oneshot::channel().0
}

impl<Resp: SendData> PartialEq for CallRef<Resp> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl<Resp: SendData> Eq for CallRef<Resp> {}

impl<Resp: SendData> Debug for CallRef<Resp> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CallRef")
            .field("target", &self.target)
            .field("id", &self.id)
            .field("deadline", &self.deadline)
            .finish()
    }
}

impl<Resp: SendData> CallRef<Resp> {
    /// Create a dummy that compares equal to its original but doesn’t actually work.
    ///
    /// This is useful within test procedures to retain a properly typed reference
    /// that can be used as argument to [`assert_respond`](crate::Effect::assert_respond)
    /// and [`resume_respond`](crate::effect_box::SimulationRunning::resume_respond).
    pub fn dummy(&self) -> Self {
        Self {
            target: self.target.clone(),
            id: self.id,
            deadline: self.deadline,
            response: oneshot::channel().0,
            _ph: PhantomData,
        }
    }

    /// Test facility for creating a fake call reference.
    pub fn fake(target: impl AsRef<str>, id: u64, deadline: Instant) -> Self {
        Self {
            target: target.as_ref().into(),
            id: CallId(id),
            deadline,
            response: oneshot::channel().0,
            _ph: PhantomData,
        }
    }

    /// Create a dummy channel for testing purposes.
    pub fn channel(deadline: Instant) -> (Self, oneshot::Receiver<Box<dyn SendData>>) {
        let (tx, rx) = oneshot::channel();
        (
            Self {
                target: Name::from("dummy"),
                id: CallId::new(),
                deadline,
                response: tx,
                _ph: PhantomData,
            },
            rx,
        )
    }
}

/// A factory for processing network stages and their wiring.
///
/// Network construction proceeds in two phases:
/// 1. create stages, providing their transition functions
/// 2. wire up the stages by providing their initial state (which usually includes [`StageRef`]s for sending to other stages)
/// 3. populate the resources collection with the necessary resources
///
/// If you forget to call [`wire_up`](StageGraph::wire_up) on a stage, the simulation will panic.
///
/// Example:
/// ```rust
/// use pure_stage::{StageGraph, tokio::TokioBuilder};
///
/// let mut network = TokioBuilder::default();
///
/// // phase 1: create stages
/// let stage = network.stage("basic", async |(mut state, out), msg: u32, eff| {
///     state += msg;
///     eff.send(&out, state).await;
///     (state, out)
/// });
/// // this is a feature of the SimulationBuilder
/// let (output, mut rx) = network.output("output", 10);
///
/// // phase 2: wire up stages by injecting targets into their state
/// let stage = network.wire_up(stage, (1u32, output));
///
/// // phase 3: populate the resources collection with the necessary resources (if used by external effects)
/// network.resources().put(42u8);
///
/// // finalize the network and run it (or make it controllable, in case of SimulationBuilder)
/// // (this needs a Tokio runtime for executing external effects)
/// let rt = tokio::runtime::Runtime::new().unwrap();
/// let mut running = network.run(rt.handle().clone());
/// ```
pub trait StageGraph {
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
    fn stage<Msg, St, F, Fut>(
        &mut self,
        name: impl AsRef<str>,
        f: F,
    ) -> StageBuildRef<Msg, St, Self::RefAux<Msg, St>>
    where
        F: FnMut(St, Msg, Effects<Msg>) -> Fut + 'static + Send,
        Fut: Future<Output = St> + 'static + Send,
        Msg: SendData + serde::de::DeserializeOwned,
        St: SendData;

    /// Finalize the given stage by providing its initial state.
    fn wire_up<Msg, St>(
        &mut self,
        stage: StageBuildRef<Msg, St, Self::RefAux<Msg, St>>,
        state: St,
    ) -> StageStateRef<Msg, St>
    where
        Msg: SendData + serde::de::DeserializeOwned,
        St: SendData;

    fn contramap<Original: SendData, Mapped: SendData>(
        &mut self,
        stage_ref: impl AsRef<StageRef<Original>>,
        new_name: impl AsRef<str>,
        transform: impl Fn(Mapped) -> Original + 'static + Send,
    ) -> StageRef<Mapped>;

    /// Preload the given stage’s mailbox with the given messages.
    ///
    /// Since the stage is not running yet, this will return false and drop messages
    /// as soon as the mailbox is full.
    ///
    /// Returns true if the messages were successfully preloaded.
    fn preload<Msg: SendData>(
        &mut self,
        stage: impl AsRef<StageRef<Msg>>,
        messages: impl IntoIterator<Item = Msg>,
    ) -> Result<(), Box<dyn SendData>>;

    /// Obtain a handle for sending messages to the given stage from outside the network.
    fn input<Msg: SendData>(&mut self, stage: impl AsRef<StageRef<Msg>>) -> Sender<Msg>;

    /// Utility function to create an output for the network.
    ///
    /// The returned [`Receiver`] can be used synchronously (e.g. in tests) or as an
    /// asynchronous [`Stream`](futures_util::Stream).
    fn output<Msg>(
        &mut self,
        name: impl AsRef<str>,
        send_queue_size: usize,
    ) -> (StageRef<Msg>, Receiver<Msg>)
    where
        Msg: SendData + PartialEq + serde::Serialize + serde::de::DeserializeOwned,
    {
        let name = Name::from(name.as_ref());
        let (sender, rx) = mpsc::channel(send_queue_size);
        let tx = MpscSender { sender };

        let output = self.stage(name, async |tx: MpscSender<Msg>, msg: Msg, eff| {
            eff.external(OutputEffect::new(eff.me().name().clone(), msg, tx.clone()))
                .await;
            tx
        });
        let output = self.wire_up(output, tx);

        (output.without_state(), Receiver::new(rx))
    }

    /// Get the resources collection for the network.
    ///
    /// It is prudent to populate this collection before running the network.
    fn resources(&self) -> &Resources;
}

/// A trait for running stage graphs.
///
/// This trait is implemented by the return value of the [`StageGraph::run`] method.
pub trait StageGraphRunning {
    /// Returns true if the stage graph has observed a termination signal.
    fn is_terminated(&self) -> bool;

    /// A future that resolves once the stage graph has terminated.
    fn termination(&self) -> BoxFuture<'static, ()>;
}

/// Generate a unique name for a stage.
///
/// This naming convention must be used by all StageGraph implementations to make them compatible
/// in terms of TraceBuffer replay.
pub fn stage_name(counter: &mut usize, prefix: &str) -> Name {
    *counter += 1;
    Name::from(&*format!("{}-{}", prefix, counter))
}
