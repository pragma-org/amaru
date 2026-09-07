// Copyright 2026 PRAGMA
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

use crate::Instant;

/// How [`super::SimulationRunning::run`] advances time and external effects.
///
/// The default stops at the next wakeup and leaves unresolved externals as [`Busy`](super::Blocked::Busy).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Run {
    pub time: TimeAdvance,
    pub externals: Externals,
}

impl Run {
    /// Advance through every wakeup until Idle/Deadlock/Terminated/Breakpoint/Busy.
    pub fn skip_wakeups() -> Self {
        Self { time: TimeAdvance::SkipWakeups, externals: Externals::LeaveBusy }
    }

    /// Skip wakeups and resolve `Busy` externals whose `run()` can complete.
    pub fn skip_and_resolve() -> Self {
        Self { time: TimeAdvance::SkipWakeups, externals: Externals::Resolve }
    }

    /// Advance time up to `until`, then stop if the next wakeup is later.
    pub fn until(until: Instant) -> Self {
        Self { time: TimeAdvance::Until(until), externals: Externals::LeaveBusy }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TimeAdvance {
    /// Stop when the only remaining work is a future wakeup. Default.
    #[default]
    StopAtWakeup,
    /// Fire due wakeups until nothing is scheduled.
    SkipWakeups,
    /// Fire wakeups up to this instant (inclusive of due-at-T).
    Until(Instant),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Externals {
    /// Leave [`Busy`](super::Blocked::Busy) for the caller (world completes `UntilResolved`).
    #[default]
    LeaveBusy,
    /// `block_on` pending effect `run()` futures while Busy.
    Resolve,
}
