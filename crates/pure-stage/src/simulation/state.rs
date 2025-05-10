use crate::{BoxFuture, Message, State};
use std::{collections::VecDeque, fmt};

pub enum InitStageState {
    Uninitialized,
    Idle(Box<dyn State>),
}

impl std::fmt::Debug for InitStageState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Uninitialized => write!(f, "Uninitialized"),
            Self::Idle(arg0) => f.debug_tuple("Idle").field(arg0).finish(),
        }
    }
}

pub type Transition = Box<
    dyn FnMut(
        Box<dyn State>,
        Box<dyn Message>,
    ) -> BoxFuture<'static, anyhow::Result<Box<dyn State>>>,
>;

pub struct InitStageData {
    pub mailbox: VecDeque<Box<dyn Message>>,
    pub state: InitStageState,
    pub transition: Transition,
}

pub enum StageState {
    Idle(Box<dyn State>),
    Running(BoxFuture<'static, anyhow::Result<Box<dyn State>>>),
    Failed,
}

impl fmt::Debug for StageState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Idle(arg0) => f.debug_tuple("Idle").field(arg0).finish(),
            Self::Running(_) => f.debug_tuple("Running").finish(),
            Self::Failed => f.debug_tuple("Failed").finish(),
        }
    }
}

pub struct StageData {
    pub mailbox: VecDeque<Box<dyn Message>>,
    pub state: StageState,
    pub transition: Transition,
}
