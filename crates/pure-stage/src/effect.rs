#![expect(clippy::expect_used)]
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

use crate::types::as_send_data_value;
use crate::{
    BoxFuture, CallId, CallRef, Instant, Name, Resources, SendData, Sender, StageRef,
    serde::{SendDataValue, never, to_cbor},
    time::Clock,
};

use crate::effect_box::{EffectBox, airlock_effect};

use crate::trace_buffer::TraceBuffer;
use cbor4ii::{core::Value, serde::from_slice};
use futures_util::FutureExt;
use parking_lot::Mutex;
use serde::de::DeserializeOwned;
use std::fmt::{Display, Error, Formatter};
use std::{
    any::{Any, type_name},
    fmt::Debug,
    future,
    marker::PhantomData,
    sync::Arc,
    time::Duration,
};
use tokio::sync::oneshot;

/// A handle for performing effects on the current stage.
///
/// This is used to send messages to other stages, wait for durations, and call other stages.
///
/// The [`StageRef`] is used to obtain a reference to the current stage, which can be used
/// in messages sent to other stages.
pub struct Effects<M> {
    me: StageRef<M>,
    effect: EffectBox,
    clock: Arc<dyn Clock + Send + Sync>,
    self_sender: Sender<M>,
    resources: Resources,
    trace_buffer: Arc<Mutex<TraceBuffer>>,
}

impl<M> Clone for Effects<M> {
    fn clone(&self) -> Self {
        Self {
            me: self.me.clone(),
            effect: self.effect.clone(),
            clock: self.clock.clone(),
            self_sender: self.self_sender.clone(),
            resources: self.resources.clone(),
            trace_buffer: self.trace_buffer.clone(),
        }
    }
}

impl<M: Debug> Debug for Effects<M> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Effects")
            .field("me", &self.me)
            .field("effect", &self.effect)
            .finish()
    }
}

impl<M: SendData> Effects<M> {
    pub(crate) fn new(
        me: StageRef<M>,
        effect: EffectBox,
        clock: Arc<dyn Clock + Send + Sync>,
        self_sender: Sender<M>,
        resources: Resources,
        trace_buffer: Arc<Mutex<TraceBuffer>>,
    ) -> Self {
        Self {
            me,
            effect,
            clock,
            self_sender,
            resources,
            trace_buffer,
        }
    }

    /// Obtain a reference to the current stage.
    ///
    /// This is useful for sending to other stages that may want to send or call back.
    pub fn me(&self) -> StageRef<M> {
        self.me.clone()
    }

    /// Obtain a reference to the current stage.
    ///
    /// Returns a borrowed reference without cloning. Use this e.g. when sending a
    /// message to the current stage. For owned access, see [`me()`](Self::me).
    pub fn me_ref(&self) -> &StageRef<M> {
        &self.me
    }

    /// Obtain a handle for sending messages to the current stage from outside the network.
    /// This allows you to perform arbitrary asynchronous tasks outside the control of the
    /// StageGraph and then feed the results into the network.
    pub fn self_sender(&self) -> Sender<M> {
        self.self_sender.clone()
    }
}

impl<M> Effects<M> {
    /// Send a message to the given stage, blocking the current stage until space has been
    /// made available in the target stage’s send queue.
    pub fn send<Msg: SendData>(&self, target: &StageRef<Msg>, msg: Msg) -> BoxFuture<'static, ()> {
        airlock_effect(
            &self.effect,
            StageEffect::Send(target.name().clone(), Box::new(msg), None),
            |_eff| Some(()),
        )
    }

    /// Obtain the current simulation time.
    pub fn clock(&self) -> BoxFuture<'static, Instant> {
        airlock_effect(&self.effect, StageEffect::Clock, |eff| match eff {
            Some(StageResponse::ClockResponse(instant)) => Some(instant),
            _ => None,
        })
    }

    /// Wait for the given duration.
    pub fn wait(&self, duration: Duration) -> BoxFuture<'static, Instant> {
        airlock_effect(&self.effect, StageEffect::Wait(duration), |eff| match eff {
            Some(StageResponse::WaitResponse(instant)) => Some(instant),
            _ => None,
        })
    }

    /// Call the given stage, blocking the current stage until the response is received.
    ///
    /// The `msg` closure is called with a reference to the call effect, which can be used
    /// to respond to the call.
    ///
    /// The returned future will resolve to `Some(resp)` if the call was successful, or `None`
    /// if the call timed out.
    pub fn call<Req: SendData, Resp: SendData + DeserializeOwned>(
        &self,
        target: &StageRef<Req>,
        timeout: Duration,
        msg: impl FnOnce(CallRef<Resp>) -> Req + Send + 'static,
    ) -> BoxFuture<'static, Option<Resp>> {
        let (response, recv) = oneshot::channel();
        let now = self.clock.now();
        let deadline = now + timeout;
        let target = target.name().clone();
        let me = self.me.name().clone();
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
                    resp.cast_deserialize::<Resp>()
                        .expect("internal messaging type error"),
                )),
                Some(StageResponse::CallTimeout) => Some(None),
                _ => None,
            },
        )
    }

    /// Respond to a call from another stage, where the call is represented by the given
    /// [`CallRef`].
    ///
    /// This effect does not block the current stage because the target of the response has been
    /// waiting for this message and is ready to receive it.
    pub fn respond<Resp: SendData>(&self, cr: CallRef<Resp>, resp: Resp) -> BoxFuture<'static, ()> {
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

    /// Run an effect that is not part of the StageGraph, as an asynchronous effect.
    pub fn external<T: ExternalEffectAPI>(&self, effect: T) -> BoxFuture<'static, T::Response> {
        airlock_effect(
            &self.effect,
            StageEffect::External(Box::new(effect)),
            |eff| match eff {
                Some(StageResponse::ExternalResponse(resp)) => Some(
                    resp.cast_deserialize::<T::Response>()
                        .expect("internal messaging type error"),
                ),
                _ => None,
            },
        )
    }

    /// Run an effect that is not part of the StageGraph as a synchronous effect.
    /// In that case we return the response directly.
    pub fn external_sync<T: ExternalEffectSync>(&self, effect: T) -> T::Response {
        self.trace_buffer
            .lock()
            .push_suspend_external(self.me.name(), &effect);
        let response = Box::new(effect)
            .run(self.resources.clone())
            .now_or_never()
            .expect("an external sync effect must complete immediately in sync context")
            .cast_deserialize::<T::Response>()
            .expect("internal messaging type error");
        self.trace_buffer
            .lock()
            .push_resume_external(self.me.name(), &response);
        response
    }

    /// Terminate this stage
    ///
    /// This will terminate this stage graph if done from a stage that was created before running the graph.
    /// This future never resolves, so you can safely return the value to exit the transition function.
    ///
    /// Example:
    ///
    /// ```ignore
    /// async |state, msg, eff| {
    ///     if msg.is_fatal() {
    ///         return eff.terminate().await;
    ///     }
    ///     // ...
    ///     state
    /// }
    /// ```
    pub fn terminate<T>(&self) -> BoxFuture<'static, T> {
        airlock_effect(&self.effect, StageEffect::Terminate, |_eff| never())
    }
}

/// A trait for effects that are not part of the StageGraph.
///
/// The [`run`](ExternalEffect::run) method is used to perform the effect unless a
/// simulator chooses differently. The latter can be done by downcasting to the concrete type.
pub trait ExternalEffect: SendData {
    /// Run the effect in production mode.
    ///
    /// Implementations typically retrieve shared services via typed lookups
    /// (e.g., `resources.get::<Arc<MyStore>>()?`).
    ///
    /// This can be overridden in simulation using [`SimulationRunning::handle_effect`](crate::effect_box::SimulationRunning::handle_effect).
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>>;

    /// Helper method for implementers of ExternalEffect.
    fn wrap(
        f: impl Future<Output = <Self as ExternalEffectAPI>::Response> + Send + 'static,
    ) -> BoxFuture<'static, Box<dyn SendData>>
    where
        Self: Sized + ExternalEffectAPI,
    {
        Box::pin(async move {
            let response = f.await;
            Box::new(response) as Box<dyn SendData>
        })
    }

    /// Helper method for implementers of ExternalEffect that have a synchronous response.
    fn wrap_sync(
        response: <Self as ExternalEffectAPI>::Response,
    ) -> BoxFuture<'static, Box<dyn SendData>>
    where
        Self: Sized + ExternalEffectAPI,
    {
        Box::pin(future::ready(Box::new(response) as Box<dyn SendData>))
    }
}

impl Display for dyn ExternalEffect {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&as_send_data_value(self).map_err(|_| Error)?.to_string())
    }
}

/// Separate trait for fixing the response type of an external effect.
///
/// This cannot be included in [`ExternalEffect`] because it would require a type parameter, which
/// in turn would make that trait non-object-safe.
pub trait ExternalEffectAPI: ExternalEffect {
    type Response: SendData + DeserializeOwned;
}

/// Marker trait for external effects that have a synchronous response.
///
/// The [`ExternalEffect::run`](ExternalEffect::run) method must return a Future that resolves
/// at the first call to `poll`. You can ensure this by not using `.await` or by using `.await`
/// only on Futures that resolve at the first call to `poll`.
pub trait ExternalEffectSync: ExternalEffectAPI {}

impl dyn ExternalEffect {
    pub fn is<T: ExternalEffect>(&self) -> bool {
        (self as &dyn Any).is::<T>()
    }

    pub fn cast_ref<T: ExternalEffect>(&self) -> Option<&T> {
        (self as &dyn Any).downcast_ref::<T>()
    }

    pub fn cast<T: ExternalEffect>(self: Box<Self>) -> anyhow::Result<Box<T>> {
        if (&*self as &dyn Any).is::<T>() {
            #[expect(clippy::expect_used)]
            Ok(Box::new(
                *(self as Box<dyn Any>)
                    .downcast::<T>()
                    .expect("checked above"),
            ))
        } else {
            anyhow::bail!(
                "external effect type error: expected {}, got {:?}",
                std::any::type_name::<T>(),
                self
            )
        }
    }
}

impl serde::Serialize for dyn ExternalEffect {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        (self as &dyn SendData).serialize(serializer)
    }
}

impl ExternalEffect for () {
    fn run(self: Box<Self>, _resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Box::pin(async { Box::new(()) as Box<dyn SendData> })
    }
}

impl ExternalEffectAPI for () {
    type Response = ();
}

/// Generic deserialization result of external effects.
///
/// External effects are serialized as part of the trace but can only generically be deserialized.
#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct UnknownExternalEffect {
    value: SendDataValue,
}

impl UnknownExternalEffect {
    pub fn new(value: SendDataValue) -> Self {
        Self { value }
    }

    pub fn value(&self) -> &Value {
        &self.value.value
    }

    pub fn send_data_value(&self) -> &SendDataValue {
        &self.value
    }

    pub fn cast<T: ExternalEffect + DeserializeOwned>(self) -> anyhow::Result<T> {
        anyhow::ensure!(
            self.value.typetag == type_name::<T>(),
            "expected `{}`, got `{}`",
            type_name::<T>(),
            self.value.typetag
        );
        let bytes = to_cbor(&self.value.value);
        Ok(from_slice(&bytes)?)
    }
}

impl ExternalEffect for UnknownExternalEffect {
    fn run(self: Box<Self>, _resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Box::pin(async { Box::new(()) as Box<dyn SendData> })
    }
}

#[test]
fn unknown_external_effect() {
    #[derive(serde::Serialize, serde::Deserialize)]
    struct Container(
        #[serde(with = "crate::serde::serialize_external_effect")] Box<dyn ExternalEffect>,
    );

    let output = crate::OutputEffect::new(Name::from("from"), 3.2, tokio::sync::mpsc::channel(1).0);
    let container = Container(Box::new(output));
    let bytes = to_cbor(&container);
    let container2: Container = from_slice(&bytes).unwrap();
    let output2 = *container2.0.cast::<UnknownExternalEffect>().unwrap();
    let output2 = output2.cast::<crate::OutputEffect<f64>>().unwrap();
    assert_eq!(output2.name, Name::from("from"));
    assert_eq!(output2.msg, 3.2);
}

/// An effect emitted by a stage (in which case T is `Box<dyn Message>`) or an effect
/// upon whose resumption the stage waits (in which case T is `()`).
#[derive(Debug)]
pub(crate) enum StageEffect<T> {
    #[cfg(feature = "simulation")]
    Receive,
    #[cfg(feature = "simulation")]
    Call(
        Name,
        Instant,
        T,
        oneshot::Receiver<Box<dyn SendData>>,
        CallId,
    ),
    Send(
        Name,
        T,
        // this is present in case the send is the first part of a call effect
        Option<(Duration, oneshot::Receiver<Box<dyn SendData>>, CallId)>,
    ),
    Clock,
    Wait(Duration),
    Respond(Name, CallId, Instant, oneshot::Sender<Box<dyn SendData>>, T),
    External(Box<dyn ExternalEffect>),
    Terminate,
}

/// The response a stage receives from the execution of an effect.
#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum StageResponse {
    Unit,
    ClockResponse(Instant),
    WaitResponse(Instant),
    CallResponse(#[serde(with = "crate::serde::serialize_send_data")] Box<dyn SendData>),
    CallTimeout,
    ExternalResponse(#[serde(with = "crate::serde::serialize_send_data")] Box<dyn SendData>),
}

impl StageResponse {
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            StageResponse::Unit => serde_json::json!({"type": "unit"}),
            StageResponse::ClockResponse(instant) => serde_json::json!({
                "type": "clock",
                "instant": instant,
            }),
            StageResponse::WaitResponse(instant) => serde_json::json!({
                "type": "wait",
                "instant": instant,
            }),
            StageResponse::CallResponse(msg) => serde_json::json!({
                "type": "call",
                "msg": format!("{msg}"),
            }),
            StageResponse::CallTimeout => serde_json::json!({"type": "timeout"}),
            StageResponse::ExternalResponse(msg) => serde_json::json!({
                "type": "external",
                "msg": format!("{msg}"),
            }),
        }
    }
}

impl Display for StageResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StageResponse::Unit => write!(f, "()"),
            StageResponse::ClockResponse(instant) => write!(f, "clock-{instant}"),
            StageResponse::WaitResponse(instant) => write!(f, "wait-{instant}"),
            StageResponse::CallResponse(msg) => {
                write!(
                    f,
                    "{msg}",
                    msg = as_send_data_value(msg.as_ref()).map_err(|_| Error)?
                )
            }
            StageResponse::CallTimeout => write!(f, "timeout"),
            StageResponse::ExternalResponse(msg) => write!(
                f,
                "{msg}",
                msg = as_send_data_value(msg.as_ref()).map_err(|_| Error)?
            ),
        }
    }
}

impl StageEffect<Box<dyn SendData>> {
    /// Split this effect from the stage into two parts:
    /// - the marker we remember in the running simulation
    /// - the effect we emit to the outside world
    #[cfg(feature = "simulation")]
    pub(crate) fn split(self, at_name: Name) -> (StageEffect<()>, Effect) {
        #[expect(clippy::panic)]
        match self {
            StageEffect::Receive => (StageEffect::Receive, Effect::Receive { at_stage: at_name }),
            StageEffect::Send(name, msg, call_param) => {
                let call = call_param
                    .as_ref()
                    .map(|(duration, _, id)| (*duration, *id));
                (
                    StageEffect::Send(name.clone(), (), call_param),
                    Effect::Send {
                        from: at_name,
                        to: name,
                        msg,
                        call,
                    },
                )
            }
            StageEffect::Clock => (StageEffect::Clock, Effect::Clock { at_stage: at_name }),
            StageEffect::Wait(duration) => (
                StageEffect::Wait(duration),
                Effect::Wait {
                    at_stage: at_name,
                    duration,
                },
            ),
            StageEffect::Call(..) => {
                panic!("call effect is only generated internally")
            }
            StageEffect::Respond(name, id, deadline, sender, msg) => (
                StageEffect::Respond(name.clone(), id, deadline, sender, ()),
                Effect::Respond {
                    at_stage: at_name,
                    target: name,
                    id,
                    msg,
                },
            ),
            StageEffect::External(effect) => (
                StageEffect::External(Box::new(())),
                Effect::External {
                    at_stage: at_name,
                    effect,
                },
            ),
            StageEffect::Terminate => (
                StageEffect::Terminate,
                Effect::Terminate { at_stage: at_name },
            ),
        }
    }
}

/// An effect emitted by a stage.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub enum Effect {
    Receive {
        at_stage: Name,
    },
    Send {
        from: Name,
        to: Name,
        #[serde(with = "crate::serde::serialize_send_data")]
        msg: Box<dyn SendData>,
        call: Option<(Duration, CallId)>,
    },
    Clock {
        at_stage: Name,
    },
    Wait {
        at_stage: Name,
        duration: Duration,
    },
    Respond {
        at_stage: Name,
        target: Name,
        id: CallId,
        #[serde(with = "crate::serde::serialize_send_data")]
        msg: Box<dyn SendData>,
    },
    External {
        at_stage: Name,
        #[serde(with = "crate::serde::serialize_external_effect")]
        effect: Box<dyn ExternalEffect>,
    },
    Terminate {
        at_stage: Name,
    },
}

impl Effect {
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            Effect::Receive { at_stage } => serde_json::json!({
                "type": "receive",
                "at_stage": at_stage,
            }),
            Effect::Send {
                from,
                to,
                msg,
                call,
            } => {
                if let Some((duration, id)) = call {
                    serde_json::json!({
                        "type": "send",
                        "from": from,
                        "to": to,
                        "msg": format!("{msg}"),
                        "call": {
                            "duration": duration.as_millis(),
                            "id": id,
                        }
                    })
                } else {
                    serde_json::json!({
                        "type": "send",
                        "from": from,
                        "to": to,
                        "msg": format!("{msg}"),
                    })
                }
            }
            Effect::Clock { at_stage } => serde_json::json!({
                "type": "clock",
                "at_stage": at_stage,
            }),
            Effect::Wait { at_stage, duration } => serde_json::json!({
                "type": "wait",
                "at_stage": at_stage,
                "duration": duration.as_millis(),
            }),
            Effect::Respond {
                at_stage,
                target,
                id,
                msg,
            } => {
                serde_json::json!({
                    "type": "respond",
                    "at_stage": at_stage,
                    "target": target,
                    "id": id,
                    "msg": format!("{msg}"),
                })
            }
            Effect::External { at_stage, effect } => {
                serde_json::json!({
                    "type": "external",
                    "at_stage": at_stage,
                    "effect": effect.to_string(),
                    "effect_type": format!("{}", as_send_data_value(effect.as_ref()).map(|v| v.typetag.clone()).unwrap_or("unknown".to_string())),
                })
            }
            Effect::Terminate { at_stage } => serde_json::json!({
                "type": "terminate",
                "at_stage": at_stage,
            }),
        }
    }
}

impl Display for Effect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Effect::Receive { at_stage } => write!(f, "receive {at_stage}"),
            Effect::Send {
                from,
                to,
                msg,
                call,
            } => {
                if let Some((duration, _id)) = call {
                    write!(
                        f,
                        "send {from} -> {to}: {msg} ({duration:?})",
                        msg = as_send_data_value(msg.as_ref()).map_err(|_| Error)?
                    )
                } else {
                    write!(
                        f,
                        "send {from} -> {to}: {msg}",
                        msg = as_send_data_value(msg.as_ref()).map_err(|_| Error)?
                    )
                }
            }
            Effect::Clock { at_stage } => write!(f, "clock {at_stage}"),
            Effect::Wait { at_stage, duration } => {
                write!(f, "wait {at_stage}: {duration:?}")
            }
            Effect::Respond {
                at_stage,
                target,
                id,
                msg,
            } => write!(
                f,
                "respond {at_stage} -> {target} {id:?}: {msg}",
                msg = as_send_data_value(msg.as_ref()).map_err(|_| Error)?
            ),
            Effect::External { at_stage, effect } => {
                write!(
                    f,
                    "external {at_stage}: {effect}",
                    effect = as_send_data_value(effect.as_ref()).map_err(|_| Error)?
                )
            }
            Effect::Terminate { at_stage } => write!(f, "terminate {at_stage}"),
        }
    }
}

#[expect(clippy::wildcard_enum_match_arm, clippy::panic)]
impl Effect {
    /// Construct a receive effect.
    pub fn receive(at_stage: impl AsRef<str>) -> Self {
        Self::Receive {
            at_stage: Name::from(at_stage.as_ref()),
        }
    }

    /// Construct a send effect.
    pub fn send(
        from: impl AsRef<str>,
        to: impl AsRef<str>,
        msg: Box<dyn SendData>,
        call: Option<(Duration, CallId)>,
    ) -> Self {
        Self::Send {
            from: Name::from(from.as_ref()),
            to: Name::from(to.as_ref()),
            msg,
            call,
        }
    }

    /// Construct a clock effect.
    pub fn clock(at_stage: impl AsRef<str>) -> Self {
        Self::Clock {
            at_stage: Name::from(at_stage.as_ref()),
        }
    }

    /// Construct a wait effect.
    pub fn wait(at_stage: impl AsRef<str>, duration: Duration) -> Self {
        Self::Wait {
            at_stage: Name::from(at_stage.as_ref()),
            duration,
        }
    }

    /// Construct a respond effect.
    pub fn respond(
        at_stage: impl AsRef<str>,
        target: impl AsRef<str>,
        id: CallId,
        msg: Box<dyn SendData>,
    ) -> Self {
        Self::Respond {
            at_stage: Name::from(at_stage.as_ref()),
            target: Name::from(target.as_ref()),
            id,
            msg,
        }
    }

    /// Construct an external effect.
    pub fn external(at_stage: impl AsRef<str>, effect: Box<dyn ExternalEffect>) -> Self {
        Self::External {
            at_stage: Name::from(at_stage.as_ref()),
            effect,
        }
    }

    /// Construct a terminate effect.
    pub fn terminate(at_stage: impl AsRef<str>) -> Self {
        Self::Terminate {
            at_stage: Name::from(at_stage.as_ref()),
        }
    }

    /// Get the stage name of this effect.
    pub fn at_stage(&self) -> &Name {
        match self {
            Effect::Receive { at_stage, .. } => at_stage,
            Effect::Send { from, .. } => from,
            Effect::Clock { at_stage, .. } => at_stage,
            Effect::Wait { at_stage, .. } => at_stage,
            Effect::Respond { at_stage, .. } => at_stage,
            Effect::External { at_stage, .. } => at_stage,
            Effect::Terminate { at_stage, .. } => at_stage,
        }
    }

    /// Assert that this effect is a receive effect.
    pub fn assert_receive<Msg>(&self, at_stage: impl AsRef<StageRef<Msg>>) {
        let at_stage = at_stage.as_ref();
        match self {
            Effect::Receive { at_stage: a } if a == at_stage.name() => {}
            _ => panic!(
                "unexpected effect {self:?}\n  looking for Receive at `{}`",
                at_stage.name()
            ),
        }
    }

    /// Assert that this effect is a send effect.
    #[expect(clippy::unwrap_used)]
    pub fn assert_send<Msg1, Msg2: SendData + PartialEq>(
        &self,
        at_stage: impl AsRef<StageRef<Msg1>>,
        target: impl AsRef<StageRef<Msg2>>,
        msg: Msg2,
    ) {
        let at_stage = at_stage.as_ref();
        let target = target.as_ref();
        match self {
            Effect::Send {
                from,
                to,
                msg: m,
                call: None,
            } if from == at_stage.name()
                && to == target.name()
                && (&**m as &dyn Any).downcast_ref::<Msg2>().unwrap() == &msg => {}
            _ => panic!(
                "unexpected effect {self:?}\n  looking for Send from `{}` to `{}` with msg {msg:?}",
                at_stage.name(),
                target.name()
            ),
        }
    }

    /// Assert that this effect is a clock effect.
    pub fn assert_clock<Msg>(&self, at_stage: impl AsRef<StageRef<Msg>>) {
        let at_stage = at_stage.as_ref();
        match self {
            Effect::Clock { at_stage: a } if a == at_stage.name() => {}
            _ => panic!(
                "unexpected effect {self:?}\n  looking for Clock at `{}`",
                at_stage.name()
            ),
        }
    }

    /// Assert that this effect is a wait effect.
    pub fn assert_wait<Msg>(&self, at_stage: impl AsRef<StageRef<Msg>>, duration: Duration) {
        let at_stage = at_stage.as_ref();
        match self {
            Effect::Wait {
                at_stage: a,
                duration: d,
            } if a == at_stage.name() && d == &duration => {}
            _ => panic!(
                "unexpected effect {self:?}\n  looking for Wait at `{}` with duration {duration:?}",
                at_stage.name()
            ),
        }
    }

    /// Assert that this effect is a call effect.
    pub fn assert_call<Msg1, Msg2: SendData, Out>(
        self,
        at_stage: impl AsRef<StageRef<Msg1>>,
        target: impl AsRef<StageRef<Msg2>>,
        extract: impl FnOnce(Msg2) -> Out,
        duration: Duration,
    ) -> Out {
        let at_stage = at_stage.as_ref();
        let target = target.as_ref();
        match self {
            Effect::Send {
                from,
                to,
                msg: m,
                call: Some((d, _id)),
            } if &from == at_stage.name() && &to == target.name() && d == duration => {
                extract(*m.cast::<Msg2>().expect("internal messaging type error"))
            }
            _ => panic!(
                "unexpected effect {self:?}\n  looking for Send from `{}` to `{}` with duration {duration:?}",
                at_stage.name(),
                target.name()
            ),
        }
    }

    /// Assert that this effect is a respond effect.
    pub fn assert_respond<Msg, Msg2: SendData + PartialEq>(
        &self,
        at_stage: impl AsRef<StageRef<Msg>>,
        cr: &CallRef<Msg2>,
        msg: Msg2,
    ) {
        let at_stage = at_stage.as_ref();
        match self {
            Effect::Respond {
                at_stage: a,
                target: _,
                id: i,
                msg: m,
            } if a == at_stage.name()
                && *i == cr.id
                && &**m as &dyn SendData == &msg as &dyn SendData => {}
            _ => panic!(
                "unexpected effect {self:?}\n  looking for Respond at `{}` with id {cr:?} and msg {msg:?}",
                at_stage.name()
            ),
        }
    }

    /// Assert that this effect is an external effect.
    pub fn assert_external<Msg, Eff: ExternalEffect + PartialEq>(
        &self,
        at_stage: impl AsRef<StageRef<Msg>>,
        effect: &Eff,
    ) {
        let at_stage = at_stage.as_ref();
        match self {
            Effect::External {
                at_stage: a,
                effect: e,
            } if a == at_stage.name() && &**e as &dyn SendData == effect as &dyn SendData => {}
            _ => panic!(
                "unexpected effect {self:?}\n  looking for External at `{}` with effect {effect:?}",
                at_stage.name()
            ),
        }
    }

    /// Extract the external effect from this effect.
    pub fn extract_external<Eff: ExternalEffectAPI + PartialEq, Msg>(
        self,
        at_stage: impl AsRef<StageRef<Msg>>,
        effect: &Eff,
    ) -> Box<Eff> {
        let at_stage = at_stage.as_ref();
        match self {
            Effect::External {
                at_stage: a,
                effect: e,
            } if &a == at_stage.name() => {
                #[expect(clippy::unwrap_used)]
                let e = e.cast::<Eff>().unwrap();
                assert_eq!(&*e, effect);
                e
            }
            _ => panic!(
                "unexpected effect {self:?}\n  looking for External at `{}` with effect {effect:?}",
                at_stage.name()
            ),
        }
    }
}

impl PartialEq for Effect {
    #[expect(clippy::wildcard_enum_match_arm)]
    fn eq(&self, other: &Self) -> bool {
        match self {
            Effect::Receive { at_stage } => match other {
                Effect::Receive {
                    at_stage: other_at_stage,
                } => at_stage == other_at_stage,
                _ => false,
            },
            Effect::Send {
                from,
                to,
                msg,
                call,
            } => match other {
                Effect::Send {
                    from: other_from,
                    to: other_to,
                    msg: other_msg,
                    call: other_call,
                } => from == other_from && to == other_to && msg == other_msg && call == other_call,
                _ => false,
            },
            Effect::Clock { at_stage } => match other {
                Effect::Clock {
                    at_stage: other_at_stage,
                } => at_stage == other_at_stage,
                _ => false,
            },
            Effect::Wait { at_stage, duration } => match other {
                Effect::Wait {
                    at_stage: other_at_stage,
                    duration: other_duration,
                } => at_stage == other_at_stage && duration == other_duration,
                _ => false,
            },
            Effect::Respond {
                at_stage,
                target,
                id,
                msg,
            } => match other {
                Effect::Respond {
                    at_stage: other_at_stage,
                    target: other_target,
                    id: other_id,
                    msg: other_msg,
                } => {
                    at_stage == other_at_stage
                        && target == other_target
                        && id == other_id
                        && msg == other_msg
                }
                _ => false,
            },
            Effect::External { at_stage, effect } => match other {
                Effect::External {
                    at_stage: other_at_stage,
                    effect: other_effect,
                } => {
                    at_stage == other_at_stage
                        && &**effect as &dyn SendData == &**other_effect as &dyn SendData
                }
                _ => false,
            },
            Effect::Terminate { at_stage } => match other {
                Effect::Terminate {
                    at_stage: other_at_stage,
                } => at_stage == other_at_stage,
                _ => false,
            },
        }
    }
}
