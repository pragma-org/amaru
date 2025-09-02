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

use super::StageEffect;
use crate::{BoxFuture, StageName, SendData};
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
    dyn FnMut(Box<dyn SendData>, Box<dyn SendData>) -> BoxFuture<'static, Box<dyn SendData>> + Send,
>;

pub struct InitStageData {
    pub mailbox: VecDeque<Box<dyn SendData>>,
    pub state: InitStageState,
    pub transition: Transition,
}

pub enum StageState {
    Idle(Box<dyn SendData>),
    Running(BoxFuture<'static, Box<dyn SendData>>),
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
    pub name: StageName,
    pub mailbox: VecDeque<Box<dyn SendData>>,
    pub state: StageState,
    pub transition: Transition,
    pub waiting: Option<StageEffect<()>>,
    pub senders: VecDeque<(StageName, Box<dyn SendData>)>,
}
