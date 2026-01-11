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

use crate::{Effect, Instant, Name};
use std::fmt;

/// Classification of why [`SimulationRunning::run_until_blocked`](crate::effect_box::SimulationRunning::run_until_blocked) has stopped.
#[derive(Debug, PartialEq)]
pub enum Blocked {
    /// All stages are suspended on [`Effect::Receive`].
    Idle,
    /// The simulation is waiting for a wakeup with no external effects pending.
    Sleeping { next_wakeup: Instant },
    /// All stages are suspended on either [`Effect::Receive`] or [`Effect::Send`].
    Deadlock(Vec<SendBlock>),
    /// The given breakpoint was hit.
    Breakpoint(Name, Effect),
    /// The given stages are suspended on effects other than [`Effect::Receive`]
    /// while none are suspended on [`Effect::Send`]. The given number of
    /// external effects are currently unresolved.
    Busy {
        external_effects: usize,
        stages: Vec<Name>,
    },
    /// The given stage has terminated.
    Terminated(Name),
}

#[derive(Debug, PartialEq)]
pub struct SendBlock {
    pub from: Name,
    pub to: Name,
    pub is_call: bool,
}

impl Blocked {
    /// Assert that the blocking reason is `Idle`.
    #[track_caller]
    pub fn assert_idle(&self) {
        match self {
            Blocked::Idle => {}
            _ => panic!("expected idle, got {:?}", self),
        }
    }

    /// Assert that the blocking reason is `Sleeping`.
    #[track_caller]
    pub fn assert_sleeping(&self) -> Instant {
        match self {
            Blocked::Sleeping { next_wakeup } => *next_wakeup,
            _ => panic!("expected sleeping, got {:?}", self),
        }
    }

    /// Assert that the blocking reason is `Sleeping` until the given instant.
    #[track_caller]
    pub fn assert_sleeping_until(&self, until: Instant) {
        match self {
            Blocked::Sleeping { next_wakeup } => {
                assert_eq!(*next_wakeup, until);
            }
            _ => panic!("expected sleeping until {:?}, got {:?}", until, self),
        }
    }

    /// Assert that the blocking reason is `Deadlock` by at least the given stages.
    #[track_caller]
    pub fn assert_deadlock(&self, names: impl IntoIterator<Item = impl AsRef<str> + fmt::Debug>) {
        let names = names.into_iter().collect::<Vec<_>>();
        match self {
            Blocked::Deadlock(deadlock)
                if deadlock
                    .iter()
                    .all(|send| names.iter().any(|n| name_match(&send.from, n.as_ref()))) => {}
            _ => panic!("expected deadlock by {:?}, got {:?}", names, self),
        }
    }

    /// Assert that the blocking reason is `Busy` by at least the given stages.
    #[track_caller]
    pub fn assert_busy(
        &self,
        names: impl IntoIterator<Item = impl AsRef<str> + fmt::Debug>,
    ) -> &Self {
        let names = names.into_iter().collect::<Vec<_>>();
        match self {
            Blocked::Busy { stages, .. }
                if names
                    .iter()
                    .all(|n| stages.iter().any(|s| name_match(s, n.as_ref()))) => {}
            _ => panic!("expected busy by {:?}, got {:?}", names, self),
        }
        self
    }

    #[track_caller]
    pub fn assert_external_effects(&self, effects: usize) {
        let Self::Busy {
            external_effects, ..
        } = self
        else {
            panic!("expected busy state but got {:?}", self);
        };
        assert_eq!(*external_effects, effects);
    }

    /// Assert that the blocking reason is `Breakpoint` by the given name.
    #[track_caller]
    pub fn assert_breakpoint(self, name: impl AsRef<str>) -> Effect {
        match self {
            Blocked::Breakpoint(n, eff) if name_match(&n, name.as_ref()) => eff,
            _ => panic!("expected breakpoint `{}`, got {:?}", name.as_ref(), self),
        }
    }

    /// Assert that the blocking reason is `Terminated` by the given name.
    #[track_caller]
    pub fn assert_terminated(self, name: impl AsRef<str>) {
        match self {
            Blocked::Terminated(n) if name_match(&n, name.as_ref()) => {}
            _ => panic!("expected terminated `{}`, got {:?}", name.as_ref(), self),
        }
    }
}

fn name_match(name: &Name, given: &str) -> bool {
    let name = name.as_str();
    if name == given {
        return true;
    }
    if name.starts_with(given)
        && name
            .get(given.len()..)
            .expect("`given` was valid UTF-8")
            .starts_with('-')
    {
        return name[given.len() + 1..].parse::<u64>().is_ok();
    }
    false
}
