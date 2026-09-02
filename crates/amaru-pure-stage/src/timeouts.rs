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

//! Coalesced protocol timers: many logical slots, at most one armed schedule.
//!
//! A due timeout is parked until the stage receives it. The next slot is armed
//! only after that receive, so many timers that share a deadline cannot flood
//! the priority mailbox.

use std::collections::BTreeMap;

use crate::{Instant, ScheduleId, SendData};

/// Logical protocol timers for one stage.
#[derive(Default)]
pub(crate) struct TimeoutHeap {
    entries: BTreeMap<u64, (Instant, Box<dyn SendData>)>,
    /// Currently sleeping for this slot, if any.
    pub armed: Option<(ScheduleId, u64)>,
    /// Fired timeout not yet consumed by receive.
    due: Option<Box<dyn SendData>>,
}

impl TimeoutHeap {
    pub fn set(&mut self, slot: u64, when: Instant, msg: Box<dyn SendData>) {
        self.entries.insert(slot, (when, msg));
    }

    pub fn clear(&mut self, slot: u64) -> bool {
        self.entries.remove(&slot).is_some()
    }

    pub fn min_slot(&self) -> Option<(u64, Instant)> {
        self.entries.iter().min_by_key(|(_, (when, _))| *when).map(|(slot, (when, _))| (*slot, *when))
    }

    pub fn has_due(&self) -> bool {
        self.due.is_some()
    }

    pub fn armed_is_current_min(&self) -> bool {
        match (self.armed, self.min_slot()) {
            (Some((id, slot)), Some((min_slot, when))) => slot == min_slot && id.time() == when,
            (None, None) => true,
            _ => false,
        }
    }

    /// Park a fired timeout for receive. Returns false if this slot is no longer armed.
    pub fn fire(&mut self, slot: u64) -> bool {
        match self.armed {
            Some((_, armed_slot)) if armed_slot == slot => {}
            _ => return false,
        }
        self.armed = None;
        let Some((_, msg)) = self.entries.remove(&slot) else {
            return false;
        };
        debug_assert!(self.due.is_none());
        self.due = Some(msg);
        true
    }

    pub fn take_due(&mut self) -> Option<Box<dyn SendData>> {
        self.due.take()
    }
}
