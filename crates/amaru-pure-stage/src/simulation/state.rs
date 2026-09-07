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

use std::{collections::VecDeque, fmt};

use crate::{BoxFuture, Name, SendData, effect::StageEffect, timeouts::TimeoutHeap};

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

pub type Transition =
    Box<dyn FnMut(Box<dyn SendData>, Box<dyn SendData>) -> BoxFuture<'static, Box<dyn SendData>> + Send>;

pub struct InitStageData {
    pub mailbox: VecDeque<Box<dyn SendData>>,
    pub state: InitStageState,
    pub transition: Transition,
}

impl fmt::Debug for InitStageData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InitStageData").field("mailbox", &self.mailbox).field("state", &self.state).finish()
    }
}

pub enum StageState {
    Idle(Box<dyn SendData>),
    Running(BoxFuture<'static, Box<dyn SendData>>),
    Terminating,
}

impl fmt::Debug for StageState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Idle(arg0) => f.debug_tuple("Idle").field(arg0).finish(),
            Self::Running(_) => f.debug_tuple("Running").finish(),
            Self::Terminating => f.debug_tuple("Terminating").finish(),
        }
    }
}

pub(crate) struct StageData {
    pub name: Name,
    /// Bounded bulk ingress (subject to `mailbox_size` and back-pressure).
    pub mailbox: VecDeque<Box<dyn SendData>>,
    /// Due self-scheduled messages, preferred over [`Self::mailbox`] on receive.
    /// Capacity is governed by the network's configured `priority_mailbox_size` together with
    /// armed timers via [`Self::scheduled_pending`].
    pub priority: VecDeque<Box<dyn SendData>>,
    pub tombstones: VecDeque<Result<Box<dyn SendData>, Name>>,
    pub state: StageState,
    pub waiting: Option<StageEffect<()>>,
    pub transition: Transition,
    pub senders: VecDeque<(Name, Box<dyn SendData>)>,
    /// Armed schedules not yet consumed by receive (including due entries still in
    /// [`Self::priority`]). Must stay ≤ the network's configured `priority_mailbox_size`.
    /// Coalesced protocol timeouts are counted via [`Self::timeouts`] instead.
    pub scheduled_pending: usize,
    pub timeouts: TimeoutHeap,
    pub supervised_by: Name,
    pub tombstone: Option<Box<dyn SendData>>,
}
