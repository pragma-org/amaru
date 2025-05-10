use crate::{Message, Name};
use std::{any::Any, fmt::Debug, time::Duration};

#[derive(Debug)]
pub(crate) enum StageEffect<T> {
    Receive,
    Send(Name, T),
    #[allow(dead_code)]
    Clock,
    #[allow(dead_code)]
    Wait(Duration),
    Interrupt,
}

impl<T: Debug> StageEffect<T> {
    pub fn assert_matching(&self, at_name: &Name, other: &Effect) -> anyhow::Result<()> {
        match self {
            StageEffect::Receive => {
                if !matches!(other, Effect::Receive { at_stage } if at_stage == at_name) {
                    anyhow::bail!("{:?} does not match {:?}", self, other)
                }
            }
            StageEffect::Send(name, _msg) => {
                // cannot check the message contents because we don’t want to clone it, so
                // the StageEffect only contains a dummy value (namely `()`)
                if !matches!(other, Effect::Send { from, to, msg: _ }
                    if from == at_name && to == name)
                {
                    anyhow::bail!("{:?} does not match {:?}", self, other)
                }
            }
            StageEffect::Clock => {
                if !matches!(other, Effect::Clock { at_stage } if at_stage == at_name) {
                    anyhow::bail!("{:?} does not match {:?}", self, other)
                }
            }
            StageEffect::Wait(dur) => {
                if !matches!(other, Effect::Wait { at_stage, duration } if at_stage==at_name && dur == duration)
                {
                    anyhow::bail!("{:?} does not match {:?}", self, other)
                }
            }
            StageEffect::Interrupt => {
                if !matches!(other, Effect::Interrupt { at_stage } if at_stage == at_name) {
                    anyhow::bail!("{:?} does not match {:?}", self, other)
                }
            }
        }
        Ok(())
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
            Effect::Interrupt { at_stage } => at_stage,
            Effect::Failure { at_stage, .. } => at_stage,
        }
    }

    pub fn assert_receive(&self, at_stage: impl AsRef<str>) {
        match self {
            Effect::Receive { at_stage: a } if a.as_str() == at_stage.as_ref() => {}
            _ => panic!("unexpected effect {self:?}"),
        }
    }

    pub fn assert_send<T: PartialEq + 'static>(
        &self,
        at_stage: impl AsRef<str>,
        target: impl AsRef<str>,
        msg: T,
    ) {
        match self {
            Effect::Send { from, to, msg: m }
                if from.as_str() == at_stage.as_ref()
                    && to.as_str() == target.as_ref()
                    && (&**m as &dyn Any).downcast_ref::<T>().unwrap() == &msg => {}
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
