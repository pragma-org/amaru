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

use std::time::{Duration, Instant};

/// Inclusive time range of log records, anchored at the last grabbed end.
///
/// Bounds are event timestamps, not view indices, so the range stays put when
/// the level filter or regex filter changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LogSelection {
    anchor: Instant,
    end: Instant,
}

impl LogSelection {
    pub(crate) fn point(at: Instant) -> Self {
        Self { anchor: at, end: at }
    }

    pub(crate) fn is_point(self) -> bool {
        self.anchor == self.end
    }

    pub(crate) fn extend(self, at: Instant) -> Self {
        Self { anchor: self.anchor, end: at }
    }

    /// Second click completes a point into a range; later clicks move the nearer end.
    pub(crate) fn click(self, at: Instant) -> Self {
        if self.is_point() { self.extend(at) } else { self.move_nearest(at) }
    }

    pub(crate) fn move_nearest(self, at: Instant) -> Self {
        if distance(at, self.start()) < distance(at, self.end()) {
            Self { anchor: self.end(), end: at }
        } else {
            Self { anchor: self.start(), end: at }
        }
    }

    pub(crate) fn start(self) -> Instant {
        self.anchor.min(self.end)
    }

    pub(crate) fn end(self) -> Instant {
        self.anchor.max(self.end)
    }

    pub(crate) fn contains(self, at: Instant) -> bool {
        at >= self.start() && at <= self.end()
    }
}

fn distance(left: Instant, right: Instant) -> Duration {
    left.max(right).duration_since(left.min(right))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn extend_covers_either_order() {
        let t0 = Instant::now() + Duration::from_secs(1);
        let t1 = t0 + Duration::from_millis(5);
        let before = t0 - Duration::from_millis(1);
        let forward = LogSelection::point(t0).extend(t1);
        let backward = LogSelection::point(t1).extend(t0);
        assert!(forward.contains(t0));
        assert!(forward.contains(t1));
        assert!(backward.contains(t0));
        assert!(backward.contains(t1));
        assert!(!forward.contains(t1 + Duration::from_millis(1)));
        assert!(!forward.contains(before));
    }

    #[test]
    fn click_extends_a_point_then_moves_the_nearer_end() {
        let t0 = Instant::now() + Duration::from_secs(1);
        let t1 = t0 + Duration::from_millis(10);
        let t2 = t0 + Duration::from_millis(20);
        let t3 = t0 + Duration::from_millis(30);

        let range = LogSelection::point(t0).click(t3);
        assert_eq!(range.start(), t0);
        assert_eq!(range.end(), t3);

        let moved_end = range.click(t2);
        assert_eq!(moved_end.start(), t0);
        assert_eq!(moved_end.end(), t2);

        let moved_start = range.click(t1);
        assert_eq!(moved_start.start(), t1);
        assert_eq!(moved_start.end(), t3);
    }
}
