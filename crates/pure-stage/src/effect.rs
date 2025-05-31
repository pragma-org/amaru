#![allow(clippy::expect_used)]

use crate::{
    simulation::{airlock_effect, EffectBox},
    BoxFuture, CallId, CallRef, Instant, Message, Name, Sender, StageRef,
};
use std::{any::Any, fmt::Debug, marker::PhantomData, sync::Arc, time::Duration};
use tokio::sync::oneshot;

pub struct Effects<M, S> {
    me: StageRef<M, S>,
    effect: EffectBox,
    now: Arc<dyn Fn() -> Instant + Send + Sync>,
    self_sender: Sender<M>,
}

impl<M, S> Clone for Effects<M, S> {
    fn clone(&self) -> Self {
        Self {
            me: self.me.clone(),
            effect: self.effect.clone(),
            now: self.now.clone(),
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

impl<M: Message, S> Effects<M, S> {
    pub(crate) fn new(
        me: StageRef<M, S>,
        effect: EffectBox,
        now: Arc<dyn Fn() -> Instant + Send + Sync>,
        self_sender: Sender<M>,
    ) -> Self {
        Self {
            me,
            effect,
            now,
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
                    *resp.cast::<Resp>().expect("internal messaging type error"),
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

    /// Run an effect that is not part of the StageGraph.
    pub fn external<T: ExternalEffectAPI>(&self, effect: T) -> BoxFuture<'static, T::Response> {
        airlock_effect(
            &self.effect,
            StageEffect::External(Box::new(effect)),
            |eff| match eff {
                Some(StageResponse::ExternalResponse(resp)) => Some(
                    *resp
                        .cast::<T::Response>()
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
pub trait ExternalEffect: Any + Debug + Send {
    /// Run the effect in production mode.
    ///
    /// This can be overridden in simulation using [`SimulationRunning::handle_effect`](crate::simulation::SimulationRunning::handle_effect).
    fn run(self: Box<Self>) -> BoxFuture<'static, Box<dyn Message>>;

    /// Compare two effects for equality in the context of testing, which may ignore some fields.
    fn test_eq(&self, other: &dyn ExternalEffect) -> bool;
}

pub trait ExternalEffectAPI: ExternalEffect {
    type Response: Message;
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

impl PartialEq for dyn ExternalEffect {
    fn eq(&self, other: &dyn ExternalEffect) -> bool {
        self.test_eq(other)
    }
}

impl ExternalEffect for () {
    fn run(self: Box<Self>) -> BoxFuture<'static, Box<dyn Message>> {
        Box::pin(async { Box::new(()) as Box<dyn Message> })
    }

    fn test_eq(&self, other: &dyn ExternalEffect) -> bool {
        self.type_id() == other.type_id()
    }
}

impl ExternalEffectAPI for () {
    type Response = ();
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
        Option<(Duration, oneshot::Receiver<Box<dyn Message>>, CallId)>,
    ),
    Clock,
    Wait(Duration),
    Call(
        Name,
        Instant,
        T,
        oneshot::Receiver<Box<dyn Message>>,
        CallId,
    ),
    Respond(Name, CallId, Instant, oneshot::Sender<Box<dyn Message>>, T),
    External(Box<dyn ExternalEffect>),
}

#[derive(Debug)]
pub(crate) enum StageResponse {
    Unit,
    ClockResponse(Instant),
    WaitResponse(Instant),
    CallResponse(Box<dyn Message>),
    CallTimeout,
    ExternalResponse(Box<dyn Message>),
}

impl StageEffect<Box<dyn Message>> {
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

#[derive(Debug)]
pub enum Effect {
    Receive {
        at_stage: Name,
    },
    Send {
        from: Name,
        to: Name,
        msg: Box<dyn Message>,
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
        msg: Box<dyn Message>,
    },
    External {
        at_stage: Name,
        effect: Box<dyn ExternalEffect>,
    },
    Failure {
        at_stage: Name,
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
    pub fn assert_send<Msg1, Msg2: Message + PartialEq, St1, St2>(
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

    pub fn assert_call<Msg1, Msg2: Message, Out, St1, St2>(
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

    pub fn assert_respond<Msg, St, Msg2: Message>(
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
            } if a == &at_stage.name && *i == cr.id && msg.eq(&**m) => {}
            _ => panic!(
                "unexpected effect {self:?}\n  looking for Respond at `{}` with id {cr:?} and msg {msg:?}",
                at_stage.name
            ),
        }
    }

    pub fn assert_external<Msg, St>(
        &self,
        at_stage: &StageRef<Msg, St>,
        effect: &dyn ExternalEffect,
    ) {
        match self {
            Effect::External {
                at_stage: a,
                effect: e,
            } if a == &at_stage.name && &**e == effect => {}
            _ => panic!(
                "unexpected effect {self:?}\n  looking for External at `{}` with effect {effect:?}",
                at_stage.name
            ),
        }
    }

    pub fn extract_external<Eff: ExternalEffectAPI, Msg, St>(
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
                assert!(
                    e.test_eq(effect),
                    "external effect mismatch: {e:?} != {effect:?}"
                );
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
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Effect::Receive { at_stage },
                Effect::Receive {
                    at_stage: other_at_stage,
                },
            ) => at_stage == other_at_stage,
            (
                Effect::Send {
                    from,
                    to,
                    msg,
                    call,
                },
                Effect::Send {
                    from: other_from,
                    to: other_to,
                    msg: other_msg,
                    call: other_call,
                },
            ) => from == other_from && to == other_to && msg == other_msg && call == other_call,
            (
                Effect::Clock { at_stage },
                Effect::Clock {
                    at_stage: other_at_stage,
                },
            ) => at_stage == other_at_stage,
            (
                Effect::Wait { at_stage, duration },
                Effect::Wait {
                    at_stage: other_at_stage,
                    duration: other_duration,
                },
            ) => at_stage == other_at_stage && duration == other_duration,
            (
                Effect::Failure { at_stage, error },
                Effect::Failure {
                    at_stage: other_at_stage,
                    error: other_error,
                },
            ) => at_stage == other_at_stage && error.to_string() == other_error.to_string(),
            _ => false,
        }
    }
}
