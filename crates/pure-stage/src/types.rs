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

use crate::{
    Effects,
    serde::{SendDataValue, to_cbor},
};
use anyhow::Context;
use cbor4ii::serde::from_slice;
use std::{
    any::{Any, type_name},
    borrow::Borrow,
    fmt,
    future::Future,
    ops::{Deref, DerefMut},
    pin::Pin,
    sync::Arc,
};
use tokio::sync::mpsc;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Type constraint for messages, which must be self-contained and have a `Debug` instance.
///
/// It is not possible to require an implementation of `PartialEq<Box<dyn Message>>`, but it
/// is possible to provide a blanket implementation for an equivalent `eq` method, which can
/// be used to manually implement PartialEq for types containing messages.
#[typetag::serialize(tag = "typetag", content = "value")]
pub trait SendData: Any + fmt::Debug + Send + 'static {
    /// Check for equality with another dynamically typed message.
    ///
    /// This is useful for implementing `PartialEq` for types containing boxed messages.
    fn test_eq(&self, other: &dyn SendData) -> bool;

    /// Deserialize the other dynamic value into this concrete type.
    fn deserialize_value(&self, other: &dyn SendData) -> anyhow::Result<Box<dyn SendData>>;
}

impl<T> SendData for T
where
    T: Any
        + PartialEq
        + fmt::Debug
        + serde::Serialize
        + serde::de::DeserializeOwned
        + Send
        + 'static,
{
    fn typetag_name(&self) -> &'static str {
        type_name::<T>()
    }

    fn test_eq(&self, other: &dyn SendData) -> bool {
        let Some(other) = (other as &dyn Any).downcast_ref::<T>() else {
            return false;
        };
        self == other
    }

    fn deserialize_value(&self, other: &dyn SendData) -> anyhow::Result<Box<dyn SendData>> {
        Ok(Box::new(deserialize_value::<T>(other)?))
    }
}

impl dyn SendData {
    /// Cast a message to a given concrete type.
    pub fn cast_ref<T: SendData>(&self) -> Option<&T> {
        (self as &dyn Any).downcast_ref::<T>()
    }

    fn try_cast<T: SendData>(self: Box<Self>) -> Result<Box<T>, Box<Self>> {
        if (&*self as &dyn Any).is::<T>() {
            #[expect(clippy::expect_used)]
            Ok(Box::new(
                *(self as Box<dyn Any>)
                    .downcast::<T>()
                    .expect("checked above"),
            ))
        } else {
            Err(self)
        }
    }

    /// Cast a message to a given concrete type, yielding an informative error otherwise
    pub fn cast<T: SendData>(self: Box<Self>) -> anyhow::Result<Box<T>> {
        self.try_cast::<T>().map_err(|b| {
            anyhow::anyhow!(
                "message type error: expected {}, got {:?} ({})",
                type_name::<T>(),
                b,
                b.typetag_name()
            )
        })
    }

    pub fn cast_deserialize<T>(self: Box<Self>) -> anyhow::Result<T>
    where
        T: SendData + serde::de::DeserializeOwned,
    {
        let this = match self.try_cast::<T>() {
            Ok(that) => return Ok(*that),
            Err(this) => this,
        };
        deserialize_value::<T>(&*this)
    }
}

fn deserialize_value<T>(this: &dyn SendData) -> anyhow::Result<T>
where
    T: SendData + serde::de::DeserializeOwned,
{
    let Some(this) = this.cast_ref::<SendDataValue>() else {
        anyhow::bail!(
            "message type error: expected {}, got {:?} ({})",
            type_name::<T>(),
            this,
            this.typetag_name()
        )
    };
    let bytes = to_cbor(&this.value);
    from_slice::<T>(&bytes).context(format!(
        "deserializing `{}` from {:?}",
        type_name::<T>(),
        this
    ))
}

impl PartialEq for dyn SendData {
    fn eq(&self, other: &dyn SendData) -> bool {
        self.test_eq(other)
    }
}

/// A unique identifier for a stage in the simulation.
///
/// This is used to identify stages in the simulation, and is used in messages sent to other stages.
/// A Name is cheap to clone and compare, and can be used as a key in a [`HashMap`](std::collections::HashMap).
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct Name(Arc<str>);

impl Name {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn append(&self, other: &str) -> Self {
        let mut new = String::with_capacity(self.0.len() + other.len());
        new.push_str(&self.0);
        new.push_str(other);
        Self(new.into())
    }
}

impl AsRef<str> for Name {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl Borrow<str> for Name {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl From<&str> for Name {
    fn from(value: &str) -> Self {
        Self(value.into())
    }
}

impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct MpscSender<T> {
    #[serde(skip, default = "dummy_sender")]
    pub sender: mpsc::Sender<T>,
}

fn dummy_sender<T>() -> mpsc::Sender<T> {
    mpsc::channel(1).0
}

impl<T: Any> fmt::Debug for MpscSender<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("MpscSender")
            .field(&std::any::type_name::<T>())
            .finish()
    }
}

impl<T: SendData> PartialEq for MpscSender<T> {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl<T> Deref for MpscSender<T> {
    type Target = mpsc::Sender<T>;

    fn deref(&self) -> &Self::Target {
        &self.sender
    }
}
impl<T> DerefMut for MpscSender<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.sender
    }
}

#[expect(dead_code)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct MpscReceiver<T> {
    #[serde(skip, default = "dummy_receiver")]
    pub receiver: mpsc::Receiver<T>,
}

#[expect(dead_code)]
fn dummy_receiver<T>() -> mpsc::Receiver<T> {
    mpsc::channel(1).1
}

impl<T: Any> fmt::Debug for MpscReceiver<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("MpscReceiver")
            .field(&std::any::type_name::<T>())
            .finish()
    }
}

impl<T: SendData> PartialEq for MpscReceiver<T> {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl<T> Deref for MpscReceiver<T> {
    type Target = mpsc::Receiver<T>;

    fn deref(&self) -> &Self::Target {
        &self.receiver
    }
}
impl<T> DerefMut for MpscReceiver<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.receiver
    }
}

/// An extension trait that allows termination or early return within a stage.
pub trait TryInStage {
    /// The successful result of this container type.
    type Result;
    /// The error type of this container type.
    type Error;

    /// Terminate the stage if the container is empty, otherwise return the contained value.
    ///
    /// # Example
    ///
    /// ```rust
    /// use pure_stage::{simulation::SimulationBuilder, StageGraph, TryInStage};
    ///
    /// let mut network = SimulationBuilder::default();
    /// network.stage("demo", async |_state: (), msg: Result<u32, String>, eff| {
    ///     let msg: u32 = msg.or_terminate(&eff, async |error: String| {
    ///         tracing::error!("error: {}", error);
    ///         // could also run effects here
    ///     })
    ///     .await;
    ///     tracing::info!("received message: {}", msg);
    /// });
    /// ```
    #[allow(async_fn_in_trait)]
    async fn or_terminate<M>(
        self,
        eff: &Effects<M>,
        alt: impl AsyncFnOnce(Self::Error),
    ) -> Self::Result;

    /// Return a value from the stage if the container is empty, otherwise continue with the contained value.
    ///
    /// This is only useful in conjunction with [`StageGraph::stage_ret`](crate::StageGraph::stage_ret).
    ///
    /// # Example
    ///
    /// ```rust
    /// use pure_stage::{simulation::SimulationBuilder, StageGraph, State, TryInStage};
    ///
    /// let mut network = SimulationBuilder::default();
    /// network.stage_ret("demo", async |state: u32, msg: Result<u32, String>, eff| {
    ///     let state: u32 = msg.or_return(async |error: String| {
    ///         tracing::error!("error: {}", error);
    ///         // could also run effects here
    ///         10
    ///     })
    ///     .await?;
    ///     tracing::info!("received message: {}", state);
    ///     State(state)
    /// });
    #[allow(async_fn_in_trait)]
    #[cfg(feature = "nightly")]
    async fn or_return<S>(
        self,
        alt: impl AsyncFnOnce(Self::Error) -> S,
    ) -> OrReturn<Self::Result, S>;
}

/// A wrapper type used with [`StageGraph::stage_ret`](crate::StageGraph::stage_ret)
/// to allow short-circuiting with the `?` operator.
pub struct WrapS<S>(pub S);

#[cfg(feature = "nightly")]
impl<S> std::ops::FromResidual<OrReturn<std::convert::Infallible, S>> for WrapS<S> {
    fn from_residual(residual: OrReturn<std::convert::Infallible, S>) -> Self {
        match residual {
            OrReturn::Return(value) => WrapS(value),
        }
    }
}

/// A wrapper type used with [`TryInStage::or_return`](crate::TryInStage::or_return)
/// to allow short-circuiting with the `?` operator with a stage crated using
/// [`StageGraph::stage_ret`](crate::StageGraph::stage_ret).
#[cfg(feature = "nightly")]
#[must_use = "this value only makes sense when followed by a `?` operator"]
pub enum OrReturn<T, S> {
    Continue(T),
    Return(S),
}

#[cfg(feature = "nightly")]
impl<T, S> std::ops::Try for OrReturn<T, S> {
    type Output = T;
    type Residual = OrReturn<std::convert::Infallible, S>;

    fn from_output(output: Self::Output) -> Self {
        OrReturn::Continue(output)
    }

    fn branch(self) -> std::ops::ControlFlow<Self::Residual, Self::Output> {
        match self {
            OrReturn::Continue(value) => std::ops::ControlFlow::Continue(value),
            OrReturn::Return(value) => std::ops::ControlFlow::Break(OrReturn::Return(value)),
        }
    }
}

#[cfg(feature = "nightly")]
impl<T, S> std::ops::FromResidual<OrReturn<std::convert::Infallible, S>> for OrReturn<T, S> {
    fn from_residual(residual: OrReturn<std::convert::Infallible, S>) -> Self {
        match residual {
            OrReturn::Return(value) => OrReturn::Return(value),
        }
    }
}

impl<T> TryInStage for Option<T> {
    type Result = T;
    type Error = ();

    async fn or_terminate<M>(
        self,
        eff: &Effects<M>,
        alt: impl AsyncFnOnce(Self::Error),
    ) -> Self::Result {
        match self {
            Some(value) => value,
            None => {
                alt(()).await;
                eff.terminate().await
            }
        }
    }

    #[cfg(feature = "nightly")]
    async fn or_return<S>(
        self,
        alt: impl AsyncFnOnce(Self::Error) -> S,
    ) -> OrReturn<Self::Result, S> {
        match self {
            Some(value) => OrReturn::Continue(value),
            None => OrReturn::Return(alt(()).await),
        }
    }
}

impl<T, E> TryInStage for Result<T, E> {
    type Result = T;

    type Error = E;

    async fn or_terminate<M>(
        self,
        eff: &Effects<M>,
        alt: impl AsyncFnOnce(Self::Error),
    ) -> Self::Result {
        match self {
            Ok(value) => value,
            Err(error) => {
                alt(error).await;
                eff.terminate().await
            }
        }
    }

    #[cfg(feature = "nightly")]
    async fn or_return<S>(
        self,
        alt: impl AsyncFnOnce(Self::Error) -> S,
    ) -> OrReturn<Self::Result, S> {
        match self {
            Ok(value) => OrReturn::Continue(value),
            Err(error) => OrReturn::Return(alt(error).await),
        }
    }
}

#[cfg(test)]
mod test {
    use crate::{
        Effect, Instant, SendData, StageGraph, StageGraphRunning, StageResponse, TryInStage, WrapS,
        serde::SendDataValue,
        simulation::SimulationBuilder,
        trace_buffer::{TraceBuffer, TraceEntry},
    };
    use std::{ffi::OsString, time::Duration};

    #[test]
    fn message() {
        let s = Box::new("hello".to_owned()) as Box<dyn SendData>;
        assert_eq!(format!("{s:?}"), "\"hello\"");
        assert_eq!(
            s.cast::<OsString>().unwrap_err().to_string(),
            "message type error: expected std::ffi::os_str::OsString, got \"hello\" (alloc::string::String)"
        );

        let s = Box::new("hello".to_owned()) as Box<dyn SendData>;
        assert_eq!(*s.cast::<String>().unwrap(), "hello");

        // the following tests show that this type of cast is robust regarding
        // auto-dereferencing, which is a common source of confusion when using
        // trait objects.

        let r0 = 1u32;
        let r1: &dyn SendData = &r0;
        let r2 = &r1;
        let r3 = &r2;

        assert_eq!(r1.cast_ref::<u32>().unwrap(), &1);
        assert_eq!(r2.cast_ref::<u32>().unwrap(), &1);
        assert_eq!(r3.cast_ref::<u32>().unwrap(), &1);

        let r0: Box<dyn SendData> = Box::new(1u32);
        let r1 = &r0;
        let r2 = &r1;
        let r3 = &r2;

        assert_eq!(r0.cast_ref::<u32>().unwrap(), &1);
        assert_eq!(r1.cast_ref::<u32>().unwrap(), &1);
        assert_eq!(r2.cast_ref::<u32>().unwrap(), &1);
        assert_eq!(r3.cast_ref::<u32>().unwrap(), &1);
    }

    #[test]
    fn try_in_stage_option() {
        let trace = TraceBuffer::new_shared(100, 1_000_000);
        let mut network = SimulationBuilder::default().with_trace_buffer(trace.clone());
        let stage = network.stage("stage", async |_: u32, msg: Option<u32>, eff| {
            msg.or_terminate(&eff, async |_| ()).await
        });
        let stage = network.wire_up(stage, 0);

        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut sim = network.run(rt.handle().clone());

        sim.enqueue_msg(&stage, [Some(1)]);
        sim.run_until_blocked();
        assert_eq!(*sim.get_state(&stage).unwrap(), 1);

        sim.enqueue_msg(&stage, [None]);
        sim.run_until_blocked();
        assert!(sim.is_terminated());

        pretty_assertions::assert_eq!(
            trace.lock().hydrate(),
            vec![
                TraceEntry::state("stage-0", SendDataValue::boxed(0u32)),
                TraceEntry::input("stage-0", SendDataValue::boxed(Some(1u32))),
                TraceEntry::resume("stage-0", StageResponse::Unit),
                TraceEntry::state("stage-0", SendDataValue::boxed(1u32)),
                TraceEntry::suspend(Effect::receive("stage-0")),
                TraceEntry::input("stage-0", SendDataValue::boxed(None::<u32>)),
                TraceEntry::resume("stage-0", StageResponse::Unit),
                TraceEntry::suspend(Effect::terminate("stage-0"))
            ]
        );
    }

    #[test]
    fn try_in_stage_result() {
        let trace = TraceBuffer::new_shared(100, 1_000_000);
        let mut network = SimulationBuilder::default().with_trace_buffer(trace.clone());
        let stage = network.stage("stage", async |_: u32, msg: Result<u32, u32>, eff| {
            msg.or_terminate(&eff, async |error| {
                eff.wait(Duration::from_secs(error.into())).await;
            })
            .await
        });
        let stage = network.wire_up(stage, 0);

        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut sim = network.run(rt.handle().clone());

        sim.enqueue_msg(&stage, [Ok(1)]);
        sim.run_until_blocked();
        assert_eq!(*sim.get_state(&stage).unwrap(), 1);

        sim.enqueue_msg(&stage, [Err(2)]);
        sim.run_until_blocked();
        assert!(sim.is_terminated());

        let two_sec = Instant::at_offset(Duration::from_secs(2));
        pretty_assertions::assert_eq!(
            trace.lock().hydrate(),
            vec![
                TraceEntry::state("stage-0", SendDataValue::boxed(0u32)),
                TraceEntry::input("stage-0", SendDataValue::boxed(Ok::<_, u32>(1u32))),
                TraceEntry::resume("stage-0", StageResponse::Unit),
                TraceEntry::state("stage-0", SendDataValue::boxed(1u32)),
                TraceEntry::suspend(Effect::receive("stage-0")),
                TraceEntry::input("stage-0", SendDataValue::boxed(Err::<u32, _>(2u32))),
                TraceEntry::resume("stage-0", StageResponse::Unit),
                TraceEntry::suspend(Effect::wait("stage-0", Duration::from_secs(2))),
                TraceEntry::clock(two_sec),
                TraceEntry::resume("stage-0", StageResponse::WaitResponse(two_sec)),
                TraceEntry::suspend(Effect::terminate("stage-0"))
            ]
        );
    }

    #[test]
    fn try_in_stage_option_return() {
        let trace = TraceBuffer::new_shared(100, 1_000_000);
        let mut network = SimulationBuilder::default().with_trace_buffer(trace.clone());
        let stage = network.stage_ret("stage", async |_: u32, msg: Option<u32>, _eff| {
            let state = msg.or_return(async |_| 10u32).await?;
            WrapS(state)
        });
        let stage = network.wire_up(stage, 0);

        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut sim = network.run(rt.handle().clone());

        sim.enqueue_msg(&stage, [Some(1)]);
        sim.run_until_blocked();
        assert_eq!(*sim.get_state(&stage).unwrap(), 1);

        sim.enqueue_msg(&stage, [None]);
        sim.run_until_blocked();
        assert!(!sim.is_terminated());
        assert_eq!(*sim.get_state(&stage).unwrap(), 10);

        pretty_assertions::assert_eq!(
            trace.lock().hydrate(),
            vec![
                TraceEntry::state("stage-0", SendDataValue::boxed(0u32)),
                TraceEntry::input("stage-0", SendDataValue::boxed(Some(1u32))),
                TraceEntry::resume("stage-0", StageResponse::Unit),
                TraceEntry::state("stage-0", SendDataValue::boxed(1u32)),
                TraceEntry::suspend(Effect::receive("stage-0")),
                TraceEntry::input("stage-0", SendDataValue::boxed(None::<u32>)),
                TraceEntry::resume("stage-0", StageResponse::Unit),
                TraceEntry::state("stage-0", SendDataValue::boxed(10u32)),
                TraceEntry::suspend(Effect::receive("stage-0"))
            ]
        );
    }

    #[test]
    fn try_in_stage_result_return() {
        let trace = TraceBuffer::new_shared(100, 1_000_000);
        let mut network = SimulationBuilder::default().with_trace_buffer(trace.clone());
        let stage = network.stage_ret("stage", async |_: u32, msg: Result<u32, u32>, eff| {
            let state = msg
                .or_return(async |error| {
                    eff.wait(Duration::from_secs(error.into())).await;
                    10
                })
                .await?;
            WrapS(state)
        });
        let stage = network.wire_up(stage, 0);

        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut sim = network.run(rt.handle().clone());

        sim.enqueue_msg(&stage, [Ok(1)]);
        sim.run_until_blocked();
        assert_eq!(*sim.get_state(&stage).unwrap(), 1);

        sim.enqueue_msg(&stage, [Err(2)]);
        sim.run_until_blocked();
        assert!(!sim.is_terminated());
        assert_eq!(*sim.get_state(&stage).unwrap(), 10);

        let two_sec = Instant::at_offset(Duration::from_secs(2));
        pretty_assertions::assert_eq!(
            trace.lock().hydrate(),
            vec![
                TraceEntry::state("stage-0", SendDataValue::boxed(0u32)),
                TraceEntry::input("stage-0", SendDataValue::boxed(Ok::<_, u32>(1u32))),
                TraceEntry::resume("stage-0", StageResponse::Unit),
                TraceEntry::state("stage-0", SendDataValue::boxed(1u32)),
                TraceEntry::suspend(Effect::receive("stage-0")),
                TraceEntry::input("stage-0", SendDataValue::boxed(Err::<u32, _>(2u32))),
                TraceEntry::resume("stage-0", StageResponse::Unit),
                TraceEntry::suspend(Effect::wait("stage-0", Duration::from_secs(2))),
                TraceEntry::clock(two_sec),
                TraceEntry::resume("stage-0", StageResponse::WaitResponse(two_sec)),
                TraceEntry::state("stage-0", SendDataValue::boxed(10u32)),
                TraceEntry::suspend(Effect::receive("stage-0")),
            ]
        );
    }
}
