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

use crate::{
    BLACKHOLE_NAME, BoxFuture, Instant, Name, Resources, ScheduleId, SendData, StageBuildRef,
    StageRef,
    effect_box::{EffectBox, airlock_effect},
    serde::{NoDebug, SendDataValue, never, to_cbor},
    time::Clock,
};

use crate::simulation::Transition;
use crate::trace_buffer::{TraceBuffer, find_next_external_resume, find_next_external_suspend};
use cbor4ii::{core::Value, serde::from_slice};
use futures_util::FutureExt;
use parking_lot::Mutex;
#[cfg(target_arch = "riscv32")]
use parking_lot::Mutex;
use serde::de::DeserializeOwned;
#[cfg(not(target_arch = "riscv32"))]
use std::sync::atomic::AtomicU64;

use std::{
    any::{Any, type_name},
    fmt::Debug,
    future,
    marker::PhantomData,
    sync::Arc,
    time::Duration,
};
use std::{
    borrow::Borrow,
    fmt::{Display, Formatter},
};

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
    resources: Resources,
    schedule_ids: ScheduleIds,
    trace_buffer: Arc<Mutex<TraceBuffer>>,
}

impl<M> Clone for Effects<M> {
    fn clone(&self) -> Self {
        Self {
            me: self.me.clone(),
            effect: self.effect.clone(),
            schedule_ids: self.schedule_ids.clone(),
            clock: self.clock.clone(),
            resources: self.resources.clone(),
            trace_buffer: self.trace_buffer.clone(),
        }
    }
}

impl<M> Debug for Effects<M> {
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
        resources: Resources,
        schedule_ids: ScheduleIds,
        trace_buffer: Arc<Mutex<TraceBuffer>>,
    ) -> Self {
        Self {
            me,
            effect,
            schedule_ids,
            clock,
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
}

#[derive(Debug, Clone)]
pub struct ScheduleIds {
    #[cfg(not(target_arch = "riscv32"))]
    counter: Arc<AtomicU64>,
    #[cfg(target_arch = "riscv32")]
    counter: Arc<parking_lot::Mutex<u64>>,
}

impl Default for ScheduleIds {
    fn default() -> Self {
        Self::new()
    }
}

impl ScheduleIds {
    pub fn new() -> Self {
        Self {
            #[cfg(not(target_arch = "riscv32"))]
            counter: Arc::new(AtomicU64::new(0)),
            #[cfg(target_arch = "riscv32")]
            counter: Arc::new(parking_lot::Mutex::new(0)),
        }
    }

    pub fn next_at(&self, instant: Instant) -> ScheduleId {
        #[cfg(not(target_arch = "riscv32"))]
        let id = self
            .counter
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        #[cfg(target_arch = "riscv32")]
        let id = {
            let mut guard = self.counter.lock();
            let id = *guard;
            *guard += 1;
            id
        };
        ScheduleId::new(id, instant)
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct CallTimeout;

impl<M> Effects<M> {
    /// Send a message to the given stage, blocking the current stage until space has been
    /// made available in the target stage’s send queue.
    pub fn send<Msg: SendData>(&self, target: &StageRef<Msg>, msg: Msg) -> BoxFuture<'static, ()> {
        let call = target.extra().cloned();
        airlock_effect(
            &self.effect,
            StageEffect::Send(target.name().clone(), call, Box::new(msg)),
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
    #[expect(clippy::panic)]
    pub fn call<Req: SendData, Resp: SendData + DeserializeOwned>(
        &self,
        target: &StageRef<Req>,
        timeout: Duration,
        msg: impl FnOnce(StageRef<Resp>) -> Req + Send + 'static,
    ) -> BoxFuture<'static, Option<Resp>> {
        if target.extra().is_some() {
            panic!(
                "cannot answer a call with a call ({} -> {})",
                self.me.name(),
                target.name()
            );
        }
        let msg = Box::new(move |name: Name, extra: Arc<dyn Any + Send + Sync>| {
            Box::new(msg(StageRef::new(name).with_extra(extra))) as Box<dyn SendData>
        }) as CallFn;
        airlock_effect(
            &self.effect,
            StageEffect::Call(
                target.name().clone(),
                timeout,
                CallExtra::CallFn(NoDebug::new(msg)),
            ),
            |eff| match eff {
                Some(StageResponse::CallResponse(resp)) => {
                    if resp.typetag_name() == type_name::<CallTimeout>() {
                        Some(None)
                    } else {
                        Some(Some(
                            resp.cast_deserialize::<Resp>()
                                .expect("internal message type error"),
                        ))
                    }
                }
                _ => None,
            },
        )
    }

    /// Schedule a message to be sent to the given stage at a specific instant in the future.
    ///
    /// Returns a token that can be used to cancel the scheduled message using [`cancel_schedule`](Self::cancel_schedule).
    ///
    /// The scheduled message will be sent at the specified `when` instant. If `when` is in the past,
    /// the message will be sent immediately.
    pub fn schedule_at(&self, msg: M, when: Instant) -> BoxFuture<'static, ScheduleId>
    where
        M: SendData,
    {
        let id = self.schedule_ids.next_at(when);
        airlock_effect(
            &self.effect,
            StageEffect::Schedule(Box::new(msg), id),
            move |eff| match eff {
                Some(StageResponse::Unit) => Some(id),
                _ => None,
            },
        )
    }

    /// Schedule a message to be sent to the given stage after a delay.
    ///
    /// Returns a token that can be used to cancel the scheduled message using [`cancel_schedule`](Self::cancel_schedule).
    ///
    /// The scheduled message will be sent after the specified `delay` duration from now.
    pub fn schedule_after(&self, msg: M, delay: Duration) -> BoxFuture<'static, ScheduleId>
    where
        M: SendData,
    {
        let now = self.clock.now();
        let when = now + delay;
        self.schedule_at(msg, when)
    }

    /// Cancel a previously scheduled message.
    ///
    /// Returns `true` if the scheduled message was found and cancelled, or `false` if it was
    /// not found (e.g., it was already sent or already cancelled).
    pub fn cancel_schedule(&self, id: ScheduleId) -> BoxFuture<'static, bool> {
        airlock_effect(
            &self.effect,
            StageEffect::CancelSchedule(id),
            |eff| match eff {
                Some(StageResponse::CancelScheduleResponse(cancelled)) => Some(cancelled),
                _ => None,
            },
        )
    }

    /// Execute a function with a timeout.
    /// If the timeout is reached before the function completes, the message is sent and can be used
    /// to terminate the stage.
    pub async fn timeout<R>(&self, msg: M, delay: Duration, f: impl Future<Output = R>) -> R
    where
        M: SendData + Sync,
        R: Send + Sync,
    {
        let schedule_id = self.schedule_after(msg, delay).await;
        let result = f.await;
        self.cancel_schedule(schedule_id).await;
        result
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
    #[allow(clippy::panic)]
    pub fn external_sync<T: ExternalEffectSync + serde::Serialize>(
        &self,
        effect: T,
    ) -> T::Response {
        self.trace_buffer
            .lock()
            .push_suspend_external(self.me.name(), &effect);

        let response = if let Some(fetch_replay) = self.trace_buffer.lock().fetch_replay_mut() {
            let Some(replayed) = find_next_external_suspend(fetch_replay, self.me.name()) else {
                panic!("no entry found in fetch replay");
            };
            let Effect::External { effect, .. } = from_slice(&to_cbor(&Effect::External {
                at_stage: self.me.name().clone(),
                effect: Box::new(effect),
            }))
            .expect("internal replay error") else {
                panic!("serde roundtrip broken");
            };

            assert!(
                replayed.test_eq(&*effect),
                "replayed effect {replayed:?}\ndoes not match performed effect {effect:?}"
            );
            find_next_external_resume(fetch_replay, self.me.name())
                .expect("no response found in fetch replay")
                .cast_deserialize::<T::Response>()
                .expect("internal messaging type error")
        } else {
            Box::new(effect)
                .run(self.resources.clone())
                .now_or_never()
                .expect("an external sync effect must complete immediately in sync context")
                .cast_deserialize::<T::Response>()
                .expect("internal messaging type error")
        };

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

    #[expect(clippy::future_not_send)]
    pub async fn stage<Msg, St, F, Fut>(
        &self,
        name: impl AsRef<str>,
        mut f: F,
    ) -> crate::StageBuildRef<Msg, St, (TransitionFactory, CanSupervise)>
    where
        F: FnMut(St, Msg, Effects<Msg>) -> Fut + 'static + Send,
        Fut: Future<Output = St> + 'static + Send,
        Msg: SendData + serde::de::DeserializeOwned,
        St: SendData,
    {
        let name = Name::from(name.as_ref());

        let name = airlock_effect(&self.effect, StageEffect::AddStage(name), |eff| match eff {
            Some(StageResponse::AddStageResponse(name)) => Some(name),
            _ => None,
        })
        .await;

        let clock = self.clock.clone();
        let resources = self.resources.clone();
        let me = StageRef::new(name.clone());
        let trace_buffer = self.trace_buffer.clone();
        let schedule_ids = self.schedule_ids.clone();

        let transition = move |effect: EffectBox| {
            let eff = Effects::new(me, effect, clock, resources, schedule_ids, trace_buffer);
            Box::new(move |state: Box<dyn SendData>, msg: Box<dyn SendData>| {
                let state = state.cast::<St>().expect("internal state type error");
                let msg = msg
                    .cast_deserialize::<Msg>()
                    .expect("internal message type error");
                let state = f(*state, msg, eff.clone());
                Box::pin(async move { Box::new(state.await) as Box<dyn SendData> })
                    as BoxFuture<'static, Box<dyn SendData>>
            }) as Transition
        };
        let can_supervise = CanSupervise(name.clone());
        crate::StageBuildRef {
            name,
            network: (Box::new(transition), can_supervise),
            _ph: PhantomData,
        }
    }

    /// Supervise the given stage by sending the tombstone when it terminates.
    ///
    /// When an unsupervised child stage terminates, this stage will terminate as well.
    pub fn supervise<Msg, St>(
        &self,
        stage: crate::StageBuildRef<Msg, St, (TransitionFactory, CanSupervise)>,
        tombstone: M,
    ) -> crate::StageBuildRef<Msg, St, (TransitionFactory, M)> {
        let StageBuildRef { name, network, .. } = stage;

        crate::StageBuildRef {
            name,
            network: (network.0, tombstone),
            _ph: PhantomData,
        }
    }

    #[expect(clippy::future_not_send)]
    pub async fn contramap<Original: SendData, Mapped: SendData>(
        &self,
        stage: impl AsRef<StageRef<Original>>,
        new_name: impl AsRef<str>,
        transform: impl Fn(Mapped) -> Original + Send + 'static,
    ) -> StageRef<Mapped> {
        let new_name = Name::from(new_name.as_ref());
        let transform = Box::new(move |mapped: Box<dyn SendData>| {
            let mapped = mapped
                .cast::<Mapped>()
                .expect("internal message type error");
            let original = transform(*mapped);
            Box::new(original) as Box<dyn SendData>
        });
        let name = airlock_effect(
            &self.effect,
            StageEffect::Contramap {
                original: stage.as_ref().name().clone(),
                new_name,
                transform: NoDebug::new(transform),
            },
            |eff| match eff {
                Some(StageResponse::ContramapResponse(name)) => Some(name),
                _ => None,
            },
        )
        .await;
        StageRef::new(name)
    }

    #[expect(clippy::future_not_send)]
    pub async fn wire_up<Msg, St, T: SendData>(
        &self,
        stage: crate::StageBuildRef<Msg, St, (TransitionFactory, T)>,
        state: St,
    ) -> StageRef<Msg>
    where
        Msg: SendData + serde::de::DeserializeOwned,
        St: SendData,
    {
        let StageBuildRef { name, network, _ph } = stage;
        let (transition, tombstone) = network;

        airlock_effect(
            &self.effect,
            StageEffect::WireStage(
                name.clone(),
                NoDebug::new(transition),
                Box::new(state),
                Box::new(tombstone),
            ),
            |eff| match eff {
                Some(StageResponse::Unit) => Some(()),
                _ => None,
            },
        )
        .await;

        StageRef::new(name)
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CanSupervise(pub(crate) Name); // private field to prevent instantiation

impl CanSupervise {
    pub fn for_test() -> Self {
        Self(BLACKHOLE_NAME.clone())
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
        let me = (self as &dyn SendData).as_send_data_value();
        write!(f, "{}", me.borrow())
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

    pub fn boxed<T: ExternalEffect>(value: &T) -> Box<dyn ExternalEffect> {
        Box::new(Self::new(SendDataValue::new(value)))
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

type CallFn =
    Box<dyn FnOnce(Name, Arc<dyn Any + Send + Sync>) -> Box<dyn SendData> + Send + 'static>;
#[derive(Debug)]
pub enum CallExtra {
    CallFn(NoDebug<CallFn>),
    Scheduled(ScheduleId),
}

/// An effect emitted by a stage (in which case T is `Box<dyn Message>`) or an effect
/// upon whose resumption the stage waits (in which case T is `()`).
#[derive(Debug)]
pub enum StageEffect<T> {
    Receive,
    Send(Name, Option<Arc<dyn Any + Send + Sync>>, T),
    Call(Name, Duration, CallExtra),
    Clock,
    Wait(Duration),
    Schedule(T, ScheduleId),
    CancelSchedule(ScheduleId),
    External(Box<dyn ExternalEffect>),
    Terminate,
    AddStage(Name),
    WireStage(Name, NoDebug<TransitionFactory>, T, T),
    Contramap {
        original: Name,
        new_name: Name,
        transform: NoDebug<Box<dyn Fn(Box<dyn SendData>) -> Box<dyn SendData> + Send + 'static>>,
    },
}

pub type TransitionFactory = Box<dyn FnOnce(EffectBox) -> Transition + Send + 'static>;

/// The response a stage receives from the execution of an effect.
#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum StageResponse {
    Unit,
    ClockResponse(Instant),
    WaitResponse(Instant),
    CallResponse(#[serde(with = "crate::serde::serialize_send_data")] Box<dyn SendData>),
    CancelScheduleResponse(bool),
    ExternalResponse(#[serde(with = "crate::serde::serialize_send_data")] Box<dyn SendData>),
    AddStageResponse(Name),
    ContramapResponse(Name),
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
            StageResponse::CancelScheduleResponse(cancelled) => serde_json::json!({
                "type": "cancel_schedule",
                "cancelled": cancelled,
            }),
            StageResponse::ExternalResponse(msg) => serde_json::json!({
                "type": "external",
                "msg": format!("{msg}"),
            }),
            StageResponse::AddStageResponse(name) => serde_json::json!({
                "type": "add_stage",
                "name": name,
            }),
            StageResponse::ContramapResponse(name) => serde_json::json!({
                "type": "contramap",
                "name": name,
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
                write!(f, "{}", msg.as_send_data_value().borrow())
            }
            StageResponse::CancelScheduleResponse(cancelled) => {
                write!(f, "cancel_schedule-{}", cancelled)
            }
            StageResponse::ExternalResponse(msg) => {
                write!(f, "{}", msg.as_send_data_value().borrow())
            }
            StageResponse::AddStageResponse(name) => write!(f, "add_stage {name}"),
            StageResponse::ContramapResponse(name) => write!(f, "contramap {name}"),
        }
    }
}

impl StageEffect<Box<dyn SendData>> {
    /// Split this effect from the stage into two parts:
    /// - the marker we remember in the running simulation
    /// - the effect we emit to the outside world
    pub(crate) fn split(
        self,
        at_name: Name,
        schedule_ids: &ScheduleIds,
        now: Instant,
    ) -> (StageEffect<()>, Effect) {
        #[expect(clippy::panic)]
        match self {
            StageEffect::Receive => (StageEffect::Receive, Effect::Receive { at_stage: at_name }),
            StageEffect::Send(name, call, msg) => {
                let is_call = call.is_some();
                (
                    StageEffect::Send(name.clone(), call, ()),
                    Effect::Send {
                        from: at_name,
                        to: name,
                        call: is_call,
                        msg,
                    },
                )
            }
            StageEffect::Call(name, duration, msg) => {
                let id = schedule_ids.next_at(now + duration);
                let CallExtra::CallFn(msg) = msg else {
                    panic!("expected CallFn, got {:?}", msg);
                };
                let msg = (msg.into_inner())(at_name.clone(), Arc::new(id));
                (
                    StageEffect::Call(name.clone(), duration, CallExtra::Scheduled(id)),
                    Effect::Call {
                        from: at_name,
                        to: name,
                        duration,
                        msg,
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
            StageEffect::Schedule(msg, id) => (
                StageEffect::Schedule((), id),
                Effect::Schedule {
                    at_stage: at_name,
                    msg,
                    id,
                },
            ),
            StageEffect::CancelSchedule(id) => (
                StageEffect::CancelSchedule(id),
                Effect::CancelSchedule {
                    at_stage: at_name,
                    id,
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
            StageEffect::AddStage(name) => (
                StageEffect::AddStage(name.clone()),
                Effect::AddStage {
                    at_stage: at_name,
                    name,
                },
            ),
            StageEffect::WireStage(name, transition, initial_state, tombstone) => (
                StageEffect::WireStage(name.clone(), transition, (), ()),
                Effect::WireStage {
                    at_stage: at_name,
                    name,
                    initial_state,
                    tombstone,
                },
            ),
            StageEffect::Contramap {
                original,
                new_name,
                transform,
            } => (
                StageEffect::Contramap {
                    original: original.clone(),
                    new_name: new_name.clone(),
                    transform,
                },
                Effect::Contramap {
                    at_stage: at_name,
                    original,
                    new_name,
                },
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
        call: bool,
        #[serde(with = "crate::serde::serialize_send_data")]
        msg: Box<dyn SendData>,
    },
    Call {
        from: Name,
        to: Name,
        duration: Duration,
        #[serde(with = "crate::serde::serialize_send_data")]
        msg: Box<dyn SendData>,
    },
    Clock {
        at_stage: Name,
    },
    Wait {
        at_stage: Name,
        duration: Duration,
    },
    Schedule {
        at_stage: Name,
        #[serde(with = "crate::serde::serialize_send_data")]
        msg: Box<dyn SendData>,
        id: ScheduleId,
    },
    CancelSchedule {
        at_stage: Name,
        id: ScheduleId,
    },
    External {
        at_stage: Name,
        #[serde(with = "crate::serde::serialize_external_effect")]
        effect: Box<dyn ExternalEffect>,
    },
    Terminate {
        at_stage: Name,
    },
    AddStage {
        at_stage: Name,
        name: Name,
    },
    WireStage {
        at_stage: Name,
        name: Name,
        #[serde(with = "crate::serde::serialize_send_data")]
        initial_state: Box<dyn SendData>,
        #[serde(with = "crate::serde::serialize_send_data")]
        tombstone: Box<dyn SendData>,
    },
    Contramap {
        at_stage: Name,
        original: Name,
        new_name: Name,
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
                call,
                msg,
            } => {
                serde_json::json!({
                    "type": "send",
                    "from": from,
                    "to": to,
                    "call": format!("{:?}", call),
                    "msg": format!("{msg}"),
                })
            }
            Effect::Call {
                from,
                to,
                duration,
                msg,
            } => serde_json::json!({
                "type": "call",
                "from": from,
                "to": to,
                "duration": duration.as_millis(),
                "msg": format!("{msg}"),
            }),
            Effect::Clock { at_stage } => serde_json::json!({
                "type": "clock",
                "at_stage": at_stage,
            }),
            Effect::Wait { at_stage, duration } => serde_json::json!({
                "type": "wait",
                "at_stage": at_stage,
                "duration": duration.as_millis(),
            }),
            Effect::Schedule { at_stage, msg, id } => serde_json::json!({
                "type": "schedule",
                "at_stage": at_stage,
                "msg": format!("{msg}"),
                "id": format!("{:?}", id),
            }),
            Effect::CancelSchedule { at_stage, id } => serde_json::json!({
                "type": "cancel_schedule",
                "at_stage": at_stage,
                "id": format!("{:?}", id),
            }),
            Effect::External { at_stage, effect } => {
                let effect_type = effect
                    .cast_ref::<UnknownExternalEffect>()
                    .map(|e| e.send_data_value().typetag.as_str())
                    .unwrap_or_else(|| effect.typetag_name());
                serde_json::json!({
                    "type": "external",
                    "at_stage": at_stage,
                    "effect": effect.to_string(),
                    "effect_type": effect_type,
                })
            }
            Effect::Terminate { at_stage } => serde_json::json!({
                "type": "terminate",
                "at_stage": at_stage,
            }),
            Effect::AddStage { at_stage, name } => serde_json::json!({
                "type": "add_stage",
                "at_stage": at_stage,
                "name": name,
            }),
            Effect::WireStage {
                at_stage,
                name,
                initial_state,
                tombstone,
            } => serde_json::json!({
                "type": "wire_stage",
                "at_stage": at_stage,
                "name": name,
                "initial_state": format!("{initial_state}"),
                "tombstone": format!("{tombstone}"),
            }),
            Effect::Contramap {
                at_stage,
                original,
                new_name,
            } => serde_json::json!({
                "type": "contramap",
                "at_stage": at_stage,
                "original": original,
                "new_name": new_name,
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
                call,
                msg,
            } => {
                write!(f, "send {from} -> {to}: {call:?} {msg}",)
            }
            Effect::Call {
                from,
                to,
                duration,
                msg,
            } => {
                write!(f, "call {from} -> {to}: {duration:?} {msg}")
            }
            Effect::Clock { at_stage } => write!(f, "clock {at_stage}"),
            Effect::Wait { at_stage, duration } => {
                write!(f, "wait {at_stage}: {duration:?}")
            }
            Effect::Schedule { at_stage, msg, id } => write!(f, "schedule {at_stage} {id}: {msg}",),
            Effect::CancelSchedule { at_stage, id } => {
                write!(f, "cancel_schedule {at_stage} {id}")
            }
            Effect::External { at_stage, effect } => {
                write!(f, "external {at_stage}: {effect}",)
            }
            Effect::Terminate { at_stage } => write!(f, "terminate {at_stage}"),
            Effect::AddStage { at_stage, name } => write!(f, "add_stage {at_stage} {name}"),
            Effect::WireStage {
                at_stage,
                name,
                initial_state,
                tombstone,
            } => write!(
                f,
                "wire_stage {at_stage} {name} {initial_state} {tombstone}"
            ),
            Effect::Contramap {
                at_stage,
                original,
                new_name,
            } => {
                write!(f, "contramap {at_stage} {original} -> {new_name}")
            }
        }
    }
}

#[expect(clippy::wildcard_enum_match_arm, clippy::panic)]
impl Effect {
    /// Construct a send effect.
    pub fn send(
        from: impl AsRef<str>,
        to: impl AsRef<str>,
        call: bool,
        msg: Box<dyn SendData>,
    ) -> Self {
        Self::Send {
            from: Name::from(from.as_ref()),
            to: Name::from(to.as_ref()),
            call,
            msg,
        }
    }

    /// Construct a call effect.
    pub fn call(
        from: impl AsRef<str>,
        to: impl AsRef<str>,
        duration: Duration,
        msg: Box<dyn SendData>,
    ) -> Self {
        Self::Call {
            from: Name::from(from.as_ref()),
            to: Name::from(to.as_ref()),
            duration,
            msg,
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

    /// Construct a schedule effect.
    pub fn schedule(
        at_stage: impl AsRef<str>,
        msg: Box<dyn SendData>,
        schedule_id: &ScheduleId,
    ) -> Self {
        Self::Schedule {
            at_stage: Name::from(at_stage.as_ref()),
            msg,
            id: *schedule_id,
        }
    }

    pub fn cancel(at_stage: impl AsRef<str>, schedule_id: &ScheduleId) -> Self {
        Self::CancelSchedule {
            at_stage: Name::from(at_stage.as_ref()),
            id: *schedule_id,
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

    /// Construct an add stage effect.
    pub fn add_stage(at_stage: impl AsRef<str>, name: impl AsRef<str>) -> Self {
        Self::AddStage {
            at_stage: Name::from(at_stage.as_ref()),
            name: Name::from(name.as_ref()),
        }
    }

    /// Construct a wire stage effect.
    pub fn wire_stage(
        at_stage: impl AsRef<str>,
        name: impl AsRef<str>,
        initial_state: Box<dyn SendData>,
        tombstone: Option<Box<dyn SendData>>,
    ) -> Self {
        Self::WireStage {
            at_stage: Name::from(at_stage.as_ref()),
            name: Name::from(name.as_ref()),
            initial_state,
            tombstone: tombstone
                .unwrap_or_else(|| SendDataValue::boxed(&CanSupervise(Name::from(name.as_ref())))),
        }
    }

    pub fn contramap(
        at_stage: impl AsRef<str>,
        original: impl AsRef<str>,
        new_name: impl AsRef<str>,
    ) -> Self {
        Self::Contramap {
            at_stage: Name::from(at_stage.as_ref()),
            original: Name::from(original.as_ref()),
            new_name: Name::from(new_name.as_ref()),
        }
    }

    /// Get the stage name of this effect.
    pub fn at_stage(&self) -> &Name {
        match self {
            Effect::Receive { at_stage, .. } => at_stage,
            Effect::Send { from, .. } => from,
            Effect::Call { from, .. } => from,
            Effect::Clock { at_stage, .. } => at_stage,
            Effect::Wait { at_stage, .. } => at_stage,
            Effect::Schedule { at_stage, .. } => at_stage,
            Effect::CancelSchedule { at_stage, .. } => at_stage,
            Effect::External { at_stage, .. } => at_stage,
            Effect::Terminate { at_stage, .. } => at_stage,
            Effect::AddStage { at_stage, .. } => at_stage,
            Effect::WireStage { at_stage, .. } => at_stage,
            Effect::Contramap { at_stage, .. } => at_stage,
        }
    }

    /// Assert that this effect is a receive effect.
    #[track_caller]
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
    #[track_caller]
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
                call: _,
                msg: m,
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
    #[track_caller]
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
    #[track_caller]
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
    #[track_caller]
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
            Effect::Call {
                from,
                to,
                duration: d,
                msg,
            } if &from == at_stage.name() && &to == target.name() && d == duration => {
                extract(*msg.cast::<Msg2>().expect("internal messaging type error"))
            }
            _ => panic!(
                "unexpected effect {self:?}\n  looking for Send from `{}` to `{}` with duration {duration:?}",
                at_stage.name(),
                target.name()
            ),
        }
    }

    /// Assert that this effect is an external effect.
    #[track_caller]
    pub fn assert_external<Eff: ExternalEffect + PartialEq>(
        &self,
        at_stage: impl AsRef<Name>,
        effect: &Eff,
    ) {
        let at_stage = at_stage.as_ref();
        match self {
            Effect::External {
                at_stage: a,
                effect: e,
            } if a == at_stage && &**e as &dyn SendData == effect as &dyn SendData => {}
            _ => panic!(
                "unexpected effect {self:?}\n  looking for External at `{}` with effect {effect:?}",
                at_stage
            ),
        }
    }

    /// Extract the external effect from this effect.
    #[track_caller]
    pub fn extract_external<Eff: ExternalEffectAPI + PartialEq>(
        self,
        at_stage: impl AsRef<Name>,
    ) -> Box<Eff> {
        let at_stage = at_stage.as_ref();
        match self {
            Effect::External {
                at_stage: a,
                effect: e,
            } if &a == at_stage =>
            {
                #[expect(clippy::unwrap_used)]
                e.cast::<Eff>().unwrap()
            }
            _ => panic!(
                "unexpected effect {self:?}\n  looking for External at `{}` with effect {}",
                at_stage,
                type_name::<Eff>()
            ),
        }
    }

    /// Assert that this effect is an add stage effect.
    #[track_caller]
    pub fn assert_add_stage<Msg>(
        &self,
        at_stage: impl AsRef<StageRef<Msg>>,
        name: impl AsRef<str>,
    ) {
        let at_stage = at_stage.as_ref();
        match self {
            Effect::AddStage {
                at_stage: a,
                name: n,
            } if a == at_stage.name() && n.as_str() == name.as_ref() => {}
            _ => panic!(
                "unexpected effect {self:?}\n  looking for AddStage at `{}` with name `{}`",
                at_stage.name(),
                name.as_ref()
            ),
        }
    }

    /// Assert that this effect is a wire stage effect.
    #[track_caller]
    pub fn assert_wire_stage<Msg, St: SendData + PartialEq>(
        &self,
        at_stage: impl AsRef<StageRef<Msg>>,
        name: impl AsRef<str>,
        initial_state: St,
    ) {
        let at_stage = at_stage.as_ref();
        match self {
            Effect::WireStage {
                at_stage: a,
                name: n,
                initial_state: i,
                tombstone: _,
            } if a == at_stage.name()
                && n.as_str() == name.as_ref()
                && i.cast_ref::<St>().expect("type error") == &initial_state => {}
            _ => panic!(
                "unexpected effect {self:?}\n  looking for WireStage at `{}` with name `{}`",
                at_stage.name(),
                name.as_ref()
            ),
        }
    }

    #[track_caller]
    pub fn extract_wire_stage<Msg, St: SendData + PartialEq>(
        &self,
        at_stage: impl AsRef<StageRef<Msg>>,
        initial_state: St,
    ) -> &Name {
        let at_stage = at_stage.as_ref();
        match self {
            Effect::WireStage {
                at_stage: a,
                name: n,
                initial_state: i,
                tombstone: _,
            } if a == at_stage.name()
                && i.cast_ref::<St>().expect("type error") == &initial_state =>
            {
                n
            }
            _ => panic!(
                "unexpected effect {self:?}\n  looking for WireStage at `{}` with initial state {initial_state:?}",
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
                call,
                msg,
            } => match other {
                Effect::Send {
                    from: other_from,
                    to: other_to,
                    call: other_call,
                    msg: other_msg,
                } => from == other_from && to == other_to && call == other_call && msg == other_msg,
                _ => false,
            },
            Effect::Call {
                from,
                to,
                duration,
                msg,
            } => match other {
                Effect::Call {
                    from: other_from,
                    to: other_to,
                    duration: other_duration,
                    msg: other_msg,
                } => {
                    from == other_from
                        && to == other_to
                        && duration == other_duration
                        && msg == other_msg
                }
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
            Effect::Schedule { at_stage, msg, id } => match other {
                Effect::Schedule {
                    at_stage: other_at_stage,
                    msg: other_msg,
                    id: other_id,
                } => at_stage == other_at_stage && msg == other_msg && id == other_id,
                _ => false,
            },
            Effect::CancelSchedule { at_stage, id } => match other {
                Effect::CancelSchedule {
                    at_stage: other_at_stage,
                    id: other_id,
                } => at_stage == other_at_stage && id == other_id,
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
            Effect::AddStage { at_stage, name } => match other {
                Effect::AddStage {
                    at_stage: other_at_stage,
                    name: other_name,
                } => at_stage == other_at_stage && name == other_name,
                _ => false,
            },
            Effect::WireStage {
                at_stage,
                name,
                initial_state,
                tombstone,
            } => match other {
                Effect::WireStage {
                    at_stage: other_at_stage,
                    name: other_name,
                    initial_state: other_initial_state,
                    tombstone: other_tombstone,
                } => {
                    at_stage == other_at_stage
                        && name == other_name
                        && initial_state == other_initial_state
                        && tombstone == other_tombstone
                }
                _ => false,
            },
            Effect::Contramap {
                at_stage,
                original,
                new_name,
            } => match other {
                Effect::Contramap {
                    at_stage: other_at_stage,
                    original: other_original,
                    new_name: other_new_name,
                } => {
                    at_stage == other_at_stage
                        && original == other_original
                        && new_name == other_new_name
                }
                _ => false,
            },
        }
    }
}
