#![allow(clippy::expect_used)]

use crate::{
    serde::{to_cbor, SendDataValue},
    simulation::{airlock_effect, EffectBox},
    time::Clock,
    BoxFuture, CallId, CallRef, Instant, Name, SendData, Sender, StageRef,
};
use cbor4ii::{core::Value, serde::from_slice};
use serde::de::DeserializeOwned;
use std::{
    any::{type_name, Any},
    fmt::Debug,
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
pub struct Effects<M, S> {
    me: StageRef<M, S>,
    effect: EffectBox,
    clock: Arc<dyn Clock + Send + Sync>,
    self_sender: Sender<M>,
}

impl<M, S> Clone for Effects<M, S> {
    fn clone(&self) -> Self {
        Self {
            me: self.me.clone(),
            effect: self.effect.clone(),
            clock: self.clock.clone(),
            self_sender: self.self_sender.clone(),
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

impl<M: SendData, S> Effects<M, S> {
    pub(crate) fn new(
        me: StageRef<M, S>,
        effect: EffectBox,
        clock: Arc<dyn Clock + Send + Sync>,
        self_sender: Sender<M>,
    ) -> Self {
        Self {
            me,
            effect,
            clock,
            self_sender,
        }
    }

    /// Obtain a reference to the current stage.
    ///
    /// This is useful for sending to other stages that may want to send or call back.
    pub fn me(&self) -> StageRef<M, S> {
        self.me.clone()
    }

    /// Obtain a handle for sending messages to the current stage from outside the network.
    /// This allows you to perform arbitrary asynchronous tasks outside the control of the
    /// StageGraph and then feed the results into the network.
    pub fn self_sender(&self) -> Sender<M> {
        self.self_sender.clone()
    }

    /// Send a message to the given stage, blocking the current stage until space has been
    /// made available in the target stage’s send queue.
    pub fn send<Msg: SendData, St>(
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
    pub fn call<Req: SendData, Resp: SendData + DeserializeOwned, St>(
        &self,
        target: &StageRef<Req, St>,
        timeout: Duration,
        msg: impl FnOnce(CallRef<Resp>) -> Req + Send + 'static,
    ) -> BoxFuture<'static, Option<Resp>> {
        let (response, recv) = oneshot::channel();
        let now = self.clock.now();
        let deadline = now + timeout;
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

    /// Run an effect that is not part of the StageGraph.
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
}

/// A trait for effects that are not part of the StageGraph.
///
/// The [`run`](ExternalEffect::run) method is used to perform the effect unless a
/// simulator chooses differently. The latter can be done by downcasting to the concrete type.
pub trait ExternalEffect: SendData {
    /// Run the effect in production mode.
    ///
    /// This can be overridden in simulation using [`SimulationRunning::handle_effect`](crate::simulation::SimulationRunning::handle_effect).
    fn run(self: Box<Self>) -> BoxFuture<'static, Box<dyn SendData>>;
}

/// Separate trait for fixing the response type of an external effect.
///
/// This cannot be included in [`ExternalEffect`] because it would require a type parameter, which
/// in turn would make that trait non-object-safe.
pub trait ExternalEffectAPI: ExternalEffect {
    type Response: SendData + DeserializeOwned;
}

impl dyn ExternalEffect {
    pub fn is<T: ExternalEffect>(&self) -> bool {
        (self as &dyn Any).is::<T>()
    }

    pub fn cast_ref<T: ExternalEffect>(&self) -> Option<&T> {
        (self as &dyn Any).downcast_ref::<T>()
    }

    pub fn cast<T: ExternalEffect>(self: Box<Self>) -> anyhow::Result<Box<T>> {
        if (&*self as &dyn Any).is::<T>() {
            #[allow(clippy::expect_used)]
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
    fn run(self: Box<Self>) -> BoxFuture<'static, Box<dyn SendData>> {
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
    fn run(self: Box<Self>) -> BoxFuture<'static, Box<dyn SendData>> {
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
    Receive,
    Send(
        Name,
        T,
        // this is present in case the send is the first part of a call effect
        Option<(Duration, oneshot::Receiver<Box<dyn SendData>>, CallId)>,
    ),
    Clock,
    Wait(Duration),
    Call(
        Name,
        Instant,
        T,
        oneshot::Receiver<Box<dyn SendData>>,
        CallId,
    ),
    Respond(Name, CallId, Instant, oneshot::Sender<Box<dyn SendData>>, T),
    External(Box<dyn ExternalEffect>),
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

impl StageEffect<Box<dyn SendData>> {
    /// Split this effect from the stage into two parts:
    /// - the marker we remember in the running simulation
    /// - the effect we emit to the outside world
    pub fn split(self, at_name: Name) -> (StageEffect<()>, Effect) {
        #[allow(clippy::panic)]
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
    Failure {
        at_stage: Name,
        #[serde(with = "crate::serde::serialize_error")]
        error: anyhow::Error,
    },
}

#[allow(clippy::wildcard_enum_match_arm, clippy::panic)]
impl Effect {
    pub fn at_stage(&self) -> &Name {
        match self {
            Effect::Receive { at_stage, .. } => at_stage,
            Effect::Send { from, .. } => from,
            Effect::Clock { at_stage, .. } => at_stage,
            Effect::Wait { at_stage, .. } => at_stage,
            Effect::Respond { at_stage, .. } => at_stage,
            Effect::External { at_stage, .. } => at_stage,
            Effect::Failure { at_stage, .. } => at_stage,
        }
    }

    pub fn assert_receive<Msg, St>(&self, at_stage: &StageRef<Msg, St>) {
        match self {
            Effect::Receive { at_stage: a } if a == &at_stage.name => {}
            _ => panic!(
                "unexpected effect {self:?}\n  looking for Receive at `{}`",
                at_stage.name
            ),
        }
    }

    #[allow(clippy::unwrap_used)]
    pub fn assert_send<Msg1, Msg2: SendData + PartialEq, St1, St2>(
        &self,
        at_stage: &StageRef<Msg1, St1>,
        target: &StageRef<Msg2, St2>,
        msg: Msg2,
    ) {
        match self {
            Effect::Send {
                from,
                to,
                msg: m,
                call: None,
            } if from == &at_stage.name
                && to == &target.name
                && (&**m as &dyn Any).downcast_ref::<Msg2>().unwrap() == &msg => {}
            _ => panic!(
                "unexpected effect {self:?}\n  looking for Send from `{}` to `{}` with msg {msg:?}",
                at_stage.name, target.name
            ),
        }
    }

    pub fn assert_clock<Msg, St>(&self, at_stage: &StageRef<Msg, St>) {
        match self {
            Effect::Clock { at_stage: a } if a == &at_stage.name => {}
            _ => panic!(
                "unexpected effect {self:?}\n  looking for Clock at `{}`",
                at_stage.name
            ),
        }
    }

    pub fn assert_wait<Msg, St>(&self, at_stage: &StageRef<Msg, St>, duration: Duration) {
        match self {
            Effect::Wait {
                at_stage: a,
                duration: d,
            } if a == &at_stage.name && d == &duration => {}
            _ => panic!(
                "unexpected effect {self:?}\n  looking for Wait at `{}` with duration {duration:?}",
                at_stage.name
            ),
        }
    }

    pub fn assert_call<Msg1, Msg2: SendData, Out, St1, St2>(
        self,
        at_stage: &StageRef<Msg1, St1>,
        target: &StageRef<Msg2, St2>,
        extract: impl FnOnce(Msg2) -> Out,
        duration: Duration,
    ) -> Out {
        match self {
            Effect::Send {
                from,
                to,
                msg: m,
                call: Some((d, _id)),
            } if from == at_stage.name && to == target.name && d == duration => {
                extract(*m.cast::<Msg2>().expect("internal messaging type error"))
            }
            _ => panic!(
                "unexpected effect {self:?}\n  looking for Send from `{}` to `{}` with duration {duration:?}",
                at_stage.name, target.name
            ),
        }
    }

    pub fn assert_respond<Msg, St, Msg2: SendData + PartialEq>(
        &self,
        at_stage: &StageRef<Msg, St>,
        cr: &CallRef<Msg2>,
        msg: Msg2,
    ) {
        match self {
            Effect::Respond {
                at_stage: a,
                target: _,
                id: i,
                msg: m,
            } if a == &at_stage.name && *i == cr.id && &**m as &dyn SendData == &msg as &dyn SendData => {}
            _ => panic!(
                "unexpected effect {self:?}\n  looking for Respond at `{}` with id {cr:?} and msg {msg:?}",
                at_stage.name
            ),
        }
    }

    pub fn assert_external<Msg, St, Eff: ExternalEffect + PartialEq>(
        &self,
        at_stage: &StageRef<Msg, St>,
        effect: &Eff,
    ) {
        match self {
            Effect::External {
                at_stage: a,
                effect: e,
            } if a == &at_stage.name && &**e as &dyn SendData == effect as &dyn SendData => {}
            _ => panic!(
                "unexpected effect {self:?}\n  looking for External at `{}` with effect {effect:?}",
                at_stage.name
            ),
        }
    }

    pub fn extract_external<Eff: ExternalEffectAPI + PartialEq, Msg, St>(
        self,
        at_stage: &StageRef<Msg, St>,
        effect: &Eff,
    ) -> Box<Eff> {
        match self {
            Effect::External {
                at_stage: a,
                effect: e,
            } if a == at_stage.name => {
                #[allow(clippy::unwrap_used)]
                let e = e.cast::<Eff>().unwrap();
                assert_eq!(&*e, effect);
                e
            }
            _ => panic!(
                "unexpected effect {self:?}\n  looking for External at `{}` with effect {effect:?}",
                at_stage.name
            ),
        }
    }
}

impl PartialEq for Effect {
    #[allow(clippy::wildcard_enum_match_arm)]
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
            Effect::Failure { at_stage, error } => match other {
                Effect::Failure {
                    at_stage: other_at_stage,
                    error: other_error,
                } => at_stage == other_at_stage && error.to_string() == other_error.to_string(),
                _ => false,
            },
        }
    }
}
