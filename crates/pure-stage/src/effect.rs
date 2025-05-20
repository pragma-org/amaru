use crate::{cast_msg, CallId, CallRef, Instant, Message, Name, StageRef};
use std::{any::Any, fmt::Debug, time::Duration};
use tokio::sync::oneshot;

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
    Interrupt,
}

#[derive(Debug)]
pub(crate) enum StageResponse {
    Unit,
    ClockResponse(Instant),
    WaitResponse(Instant),
    CallResponse(Box<dyn Message>),
    CallTimeout,
}

impl StageEffect<Box<dyn Message>> {
    /// Split this effect from the stage into two parts:
    /// - the marker we remember in the running simulation
    /// - the effect we emit to the outside world
    pub fn split(self, at_name: Name) -> (StageEffect<()>, Effect) {
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
            StageEffect::Interrupt => (
                StageEffect::Interrupt,
                Effect::Interrupt { at_stage: at_name },
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
    Interrupt {
        at_stage: Name,
    },
    Failure {
        at_stage: Name,
        error: anyhow::Error,
    },
}

#[allow(clippy::wildcard_enum_match_arm)]
impl Effect {
    pub fn at_stage(&self) -> &Name {
        match self {
            Effect::Receive { at_stage, .. } => at_stage,
            Effect::Send { from, .. } => from,
            Effect::Clock { at_stage, .. } => at_stage,
            Effect::Wait { at_stage, .. } => at_stage,
            Effect::Respond { at_stage, .. } => at_stage,
            Effect::Interrupt { at_stage } => at_stage,
            Effect::Failure { at_stage, .. } => at_stage,
        }
    }

    pub fn assert_receive<Msg, St>(&self, at_stage: &StageRef<Msg, St>) {
        match self {
            Effect::Receive { at_stage: a } if a == &at_stage.name => {}
            _ => panic!("unexpected effect {self:?}\n  looking for Receive at {at_stage:?}"),
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
            _ => panic!("unexpected effect {self:?}\n  looking for Send from {at_stage:?} to {target:?} with msg {msg:?}"),
        }
    }

    pub fn assert_clock<Msg, St>(&self, at_stage: &StageRef<Msg, St>) {
        match self {
            Effect::Clock { at_stage: a } if a == &at_stage.name => {}
            _ => panic!("unexpected effect {self:?}\n  looking for Clock at {at_stage:?}"),
        }
    }

    pub fn assert_wait<Msg, St>(&self, at_stage: &StageRef<Msg, St>, duration: Duration) {
        match self {
            Effect::Wait {
                at_stage: a,
                duration: d,
            } if a == &at_stage.name && d == &duration => {}
            _ => panic!("unexpected effect {self:?}\n  looking for Wait at {at_stage:?} with duration {duration:?}"),
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
                extract(cast_msg(m).expect("internal messaging type error"))
            }
            _ => panic!("unexpected effect {self:?}\n  looking for Send from {at_stage:?} to {target:?} with duration {duration:?}"),
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
            _ => panic!("unexpected effect {self:?}\n  looking for Respond at {at_stage:?} with id {cr:?} and msg {msg:?}"),
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
            ) => {
                from == other_from && to == other_to && msg.eq(&**other_msg) && *call == *other_call
            }
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
