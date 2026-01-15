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

use crate::simulation::SimulationRunning;
use crate::{Instant, ScheduleId};
use std::collections::{BTreeMap, BTreeSet};

/// A collection of scheduled runnables.
/// It maintains an order based on the scheduled time in order to efficiently retrieve the next runnables
/// to wake up.
/// It also a has methods to remove runnables based on their ScheduleId.
pub struct ScheduledRunnables {
    by_id: BTreeMap<ScheduleId, Runnable>,
    by_time: BTreeMap<Instant, BTreeSet<ScheduleId>>,
}

type Runnable = Box<dyn FnOnce(&mut SimulationRunning) + Send + 'static>;

impl ScheduledRunnables {
    pub fn new() -> Self {
        Self {
            by_id: BTreeMap::new(),
            by_time: BTreeMap::new(),
        }
    }

    /// Return the number of scheduled runnables.
    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    /// Return true if there is a runnable with the given ScheduleId.
    pub fn contains(&self, id: &ScheduleId) -> bool {
        self.by_id.contains_key(id)
    }

    /// Return the set of runnables at the first available time that is less than or equal to `max_time`.
    /// Also return the wakeup time for those runnables or max_time if no runnables can be returned.
    pub fn wakeup(&mut self, max_time: Option<Instant>) -> (Vec<Runnable>, Option<Instant>) {
        let ids: BTreeSet<ScheduleId> = self
            .by_time
            .first_key_value()
            .filter(|(k, _)| max_time.map(|t| *k <= &t).unwrap_or(true))
            .map(|(_, ids)| ids.clone())
            .unwrap_or_default();

        let wakeup_time = ids.last().map(|id| id.time()).or(max_time);
        let mut wakeups = Vec::new();
        for id in ids {
            if let Some(runnable) = self.remove(&id) {
                wakeups.push(runnable);
            }
        }
        (wakeups, wakeup_time)
    }

    /// Add a new runnable with its ScheduleId.
    pub fn schedule(
        &mut self,
        id: ScheduleId,
        runnable: Box<dyn FnOnce(&mut SimulationRunning) + Send + 'static>,
    ) {
        self.by_id.insert(id, runnable);
        self.by_time.entry(id.time()).or_default().insert(id);
    }

    /// Return the next wakeup time of the scheduled runnables.
    pub fn next_wakeup_time(&self) -> Option<Instant> {
        self.by_time.first_key_value().map(|(k, _)| *k)
    }

    /// Remove a scheduled runnable by its ScheduleId.
    /// Return the runnable if it was found, None otherwise.
    pub fn remove(&mut self, id: &ScheduleId) -> Option<Runnable> {
        match self.by_id.remove(id) {
            Some(runnable) => {
                let time = id.time();
                if let Some(set) = self.by_time.get_mut(&time) {
                    set.remove(id);
                    if set.is_empty() {
                        // Remove the empty bucket.
                        self.by_time.remove(&time);
                    }
                }
                Some(runnable)
            }
            None => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ScheduleIds;
    use std::time::Duration;

    #[test]
    fn next_wakeup_time_is_the_smallest_time() {
        let mut sr = ScheduledRunnables::new();
        let ids = ScheduleIds::default();

        let base = Instant::now();
        let t1 = base + Duration::from_secs(10);
        let t2 = base + Duration::from_secs(5);
        let t3 = base + Duration::from_secs(20);

        let id1 = ids.next_at(t1);
        let id2 = ids.next_at(t2);
        let id3 = ids.next_at(t3);

        schedule(&mut sr, id1);
        schedule(&mut sr, id2);
        schedule(&mut sr, id3);

        assert_eq!(sr.len(), 3);
        assert_eq!(sr.next_wakeup_time(), Some(t2));
    }

    #[test]
    fn wakeup_with_no_max_time_only_removes_the_first_runnables_and_their_time() {
        let mut sr = ScheduledRunnables::new();
        let ids = ScheduleIds::default();

        let base = Instant::now();
        let t1 = base + Duration::from_secs(1);
        let t2 = base + Duration::from_secs(1);
        let t3 = base + Duration::from_secs(2);
        let t4 = base + Duration::from_secs(5);

        let id1 = ids.next_at(t1);
        let id2 = ids.next_at(t2);
        let id3 = ids.next_at(t3);
        let id4 = ids.next_at(t4);

        schedule(&mut sr, id1);
        schedule(&mut sr, id2);
        schedule(&mut sr, id3);
        schedule(&mut sr, id4);

        let (wakeups, returned_time) = sr.wakeup(None);

        assert_eq!(wakeups.len(), 2);
        assert_eq!(returned_time, Some(t2));
        assert_eq!(sr.len(), 2);
        assert_eq!(sr.next_wakeup_time(), Some(t3));
    }

    #[test]
    fn wakeup_with_max_time_only_removes_only_the_first_runnables() {
        let mut sr = ScheduledRunnables::new();
        let ids = ScheduleIds::default();

        let base = Instant::now();
        let t1 = base + Duration::from_secs(1);
        let t2 = base + Duration::from_secs(1);
        let t3 = base + Duration::from_secs(2);
        let t4 = base + Duration::from_secs(5);
        let max_time = base + Duration::from_secs(2);

        let id1 = ids.next_at(t1);
        let id2 = ids.next_at(t2);
        let id3 = ids.next_at(t3);
        let id4 = ids.next_at(t4);

        schedule(&mut sr, id1);
        schedule(&mut sr, id2);
        schedule(&mut sr, id3);
        schedule(&mut sr, id4);

        let (wakeups, returned_time) = sr.wakeup(Some(max_time));

        assert_eq!(wakeups.len(), 2);
        assert_eq!(returned_time, Some(t2));
        assert_eq!(sr.len(), 2);
        assert_eq!(sr.next_wakeup_time(), Some(t3));
    }

    #[test]
    fn wakeup_with_max_time_and_no_due_entries_returns_empty_and_max_time() {
        let mut sr = ScheduledRunnables::new();
        let ids = ScheduleIds::default();

        let base = Instant::now();
        let t1 = base + Duration::from_secs(1);
        let t2 = base + Duration::from_secs(1);
        let t3 = base + Duration::from_secs(2);
        let t4 = base + Duration::from_secs(5);

        let id1 = ids.next_at(t1);
        let id2 = ids.next_at(t2);
        let id3 = ids.next_at(t3);
        let id4 = ids.next_at(t4);

        schedule(&mut sr, id1);
        schedule(&mut sr, id2);
        schedule(&mut sr, id3);
        schedule(&mut sr, id4);

        let (wakeups, returned_time) = sr.wakeup(Some(base));

        assert!(wakeups.is_empty());
        assert_eq!(returned_time, Some(base));
        assert_eq!(sr.len(), 4);
        assert_eq!(sr.next_wakeup_time(), Some(t1));
    }

    #[test]
    fn remove_by_schedule_id_removes_only_the_matching_entry_even_with_same_time() {
        let mut sr = ScheduledRunnables::new();
        let ids = ScheduleIds::default();

        let base = Instant::now();
        let t = base + Duration::from_secs(1);

        let id_a = ids.next_at(t);
        let id_b = ids.next_at(t);

        schedule(&mut sr, id_a);
        schedule(&mut sr, id_b);

        assert_eq!(sr.len(), 2);
        assert!(sr.contains(&id_a));
        assert!(sr.contains(&id_b));

        assert!(sr.remove(&id_a).is_some());
        assert_eq!(sr.len(), 1);
        assert!(!sr.contains(&id_a));
        assert!(sr.contains(&id_b));

        assert!(sr.remove(&id_a).is_none());
        assert!(sr.remove(&id_b).is_some());
        assert_eq!(sr.len(), 0);
    }

    // HELPERS

    fn schedule(sr: &mut ScheduledRunnables, id: ScheduleId) {
        sr.schedule(
            id,
            Box::new(|_sim: &mut SimulationRunning| {
                // no-op
            }),
        );
    }
}
