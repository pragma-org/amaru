use super::Instant;
use crate::{Message, Name, StageRef};
use std::{any::Any, fmt::Debug, time::Duration};
use tokio::sync::oneshot;

/// An effect emitted by a stage (in which case T is `Box<dyn Message>`) or an effect
/// upon whose resumption the stage waits (in which case T is `()`).
#[derive(Debug)]
pub(crate) enum StageEffect<T> {
    Receive,
    Send(Name, T),
    Clock,
    Wait(Duration),
    Call(Name, Duration, T, oneshot::Receiver<Box<dyn Message>>),
    Interrupt,
}

#[derive(Debug)]
pub(crate) enum StageResponse<T> {
    Unit,
    ClockResponse(Instant),
    WaitResponse(Instant),
    CallResponse(T),
    CallTimeout,
}

impl StageEffect<Box<dyn Message>> {
    /// Split this effect from the stage into two parts:
    /// - the marker we remember in the running simulation
    /// - the effect we emit to the outside world
    pub fn to_effect(self, at_name: Name) -> (StageEffect<()>, Effect) {
        match self {
            StageEffect::Receive => (StageEffect::Receive, Effect::Receive { at_stage: at_name }),
            StageEffect::Send(name, msg) => (
                StageEffect::Send(name.clone(), ()),
                Effect::Send {
                    from: at_name,
                    to: name,
                    msg,
                },
            ),
            StageEffect::Clock => (StageEffect::Clock, Effect::Clock { at_stage: at_name }),
            StageEffect::Wait(duration) => (
                StageEffect::Wait(duration),
                Effect::Wait {
                    at_stage: at_name,
                    duration,
                },
            ),
            StageEffect::Call(name, timeout, msg, receiver) => (
                StageEffect::Call(name.clone(), timeout, (), receiver),
                Effect::Call {
                    at_stage: at_name,
                    target: name,
                    timeout,
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
    },
    Clock {
        at_stage: Name,
    },
    Wait {
        at_stage: Name,
        duration: Duration,
    },
    Call {
        at_stage: Name,
        target: Name,
        timeout: Duration,
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

impl Effect {
    pub fn at_stage(&self) -> &Name {
        match self {
            Effect::Receive { at_stage, .. } => at_stage,
            Effect::Send { from, .. } => from,
            Effect::Clock { at_stage, .. } => at_stage,
            Effect::Wait { at_stage, .. } => at_stage,
            Effect::Call { at_stage, .. } => at_stage,
            Effect::Interrupt { at_stage } => at_stage,
            Effect::Failure { at_stage, .. } => at_stage,
        }
    }

    pub fn assert_receive<Msg, St>(&self, at_stage: &StageRef<Msg, St>) {
        match self {
            Effect::Receive { at_stage: a } if a == &at_stage.name => {}
            _ => panic!("unexpected effect {self:?}"),
        }
    }

    pub fn assert_send<Msg1, Msg2: Message + PartialEq, St1, St2>(
        &self,
        at_stage: &StageRef<Msg1, St1>,
        target: &StageRef<Msg2, St2>,
        msg: Msg2,
    ) {
        match self {
            Effect::Send { from, to, msg: m }
                if from == &at_stage.name
                    && to == &target.name
                    && (&**m as &dyn Any).downcast_ref::<Msg2>().unwrap() == &msg => {}
            _ => panic!("unexpected effect {self:?}"),
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
                Effect::Send { from, to, msg },
                Effect::Send {
                    from: other_from,
                    to: other_to,
                    msg: other_msg,
                },
            ) => from == other_from && to == other_to && msg.eq(&**other_msg),
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
