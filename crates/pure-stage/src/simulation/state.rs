use super::StageEffect;
use crate::{BoxFuture, Name, SendData};
use std::{collections::VecDeque, fmt};

pub enum InitStageState {
    Uninitialized,
    Idle(Box<dyn SendData>),
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
        Box<dyn SendData>,
        Box<dyn SendData>,
    ) -> BoxFuture<'static, anyhow::Result<Box<dyn SendData>>>,
>;

pub struct InitStageData {
    pub mailbox: VecDeque<Box<dyn SendData>>,
    pub state: InitStageState,
    pub transition: Transition,
}

pub enum StageState {
    Idle(Box<dyn SendData>),
    Running(BoxFuture<'static, anyhow::Result<Box<dyn SendData>>>),
    Failed(String),
}

impl fmt::Debug for StageState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Idle(arg0) => f.debug_tuple("Idle").field(arg0).finish(),
            Self::Running(_) => f.debug_tuple("Running").finish(),
            Self::Failed(error) => f.debug_tuple("Failed").field(error).finish(),
        }
    }
}

pub struct StageData {
    pub name: Name,
    pub mailbox: VecDeque<Box<dyn SendData>>,
    pub state: StageState,
    pub transition: Transition,
    pub waiting: Option<StageEffect<()>>,
    pub senders: VecDeque<(Name, Box<dyn SendData>)>,
}
