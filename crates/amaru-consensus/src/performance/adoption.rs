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

//! Sync-only block adoption pace.
//!
//! The rate is the throughput of the last [`ADOPTION_PACE_WINDOW`] sync adoptions,
//! `(n − 1) / (last − first)`. Live adoptions clear that window, so the speed reads as zero.
//! A node that was adopting quickly and then waits longer than the average gap in the window
//! is treated as stopped.

use std::{collections::VecDeque, time::Duration};

use amaru_pure_stage::Instant;

/// Adoptions per second above which sync is still moving fast enough to suppress a lagging error.
pub const FAST_SYNC_ADOPTIONS_PER_SEC: f64 = 10.0;

/// How many recent sync adoptions form the throughput window.
const ADOPTION_PACE_WINDOW: usize = 200;

/// Recent sync adoption times, oldest first.
#[derive(Debug, Clone, PartialEq)]
pub struct SyncAdoptionPace {
    recent: VecDeque<Instant>,
}

impl Default for SyncAdoptionPace {
    fn default() -> Self {
        Self { recent: VecDeque::with_capacity(ADOPTION_PACE_WINDOW) }
    }
}

impl SyncAdoptionPace {
    /// Record an adoption. `live` clears the sync window.
    pub fn record(&mut self, at: Instant, live: bool) {
        if live {
            self.recent.clear();
            return;
        }
        if self.recent.len() == ADOPTION_PACE_WINDOW {
            self.recent.pop_front();
        }
        self.recent.push_back(at);
    }

    /// Adoptions per second over the stored window. `None` until two adoptions span a non-zero time.
    fn rate_per_sec(&self) -> Option<f64> {
        let (&first, &last) = self.recent.front().zip(self.recent.back())?;
        let span = last.saturating_since(first);
        let gaps = self.recent.len().checked_sub(1)?;
        if gaps == 0 || span.is_zero() {
            return None;
        }
        Some(gaps as f64 / span.as_secs_f64())
    }

    /// Sync adoptions are still arriving faster than [`FAST_SYNC_ADOPTIONS_PER_SEC`], and the next
    /// one is not yet overdue relative to the average gap in the window. Live mode and a stalled
    /// sync both return false.
    pub fn is_catching_up_fast(&self, now: Instant) -> bool {
        let Some(rate) = self.rate_per_sec() else {
            return false;
        };
        if rate <= FAST_SYNC_ADOPTIONS_PER_SEC {
            return false;
        }
        let Some(&last) = self.recent.back() else {
            return false;
        };
        let expected = Duration::from_secs_f64(1.0 / rate) + Duration::from_secs(1);
        now.saturating_since(last) <= expected
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(millis: u64) -> Instant {
        Instant::at_offset(Duration::from_millis(millis), Duration::ZERO)
    }

    #[test]
    fn live_adoption_reports_zero_speed() {
        let mut pace = SyncAdoptionPace::default();
        pace.record(t(0), false);
        pace.record(t(50), false);
        assert!(pace.is_catching_up_fast(t(50)));

        pace.record(t(60), true);
        assert_eq!(pace.rate_per_sec(), None);
        assert!(!pace.is_catching_up_fast(t(60)));
    }

    #[test]
    fn one_short_gap_does_not_set_the_expected_interval() {
        let mut pace = SyncAdoptionPace::default();
        // Seven 50ms gaps (20/s) and one 1ms gap. Throughput stays near 20/s.
        for step in 0..7 {
            pace.record(t(step * 50), false);
        }
        pace.record(t(7 * 50 + 1), false);
        let rate = pace.rate_per_sec().expect("window spans time");
        assert!(rate > FAST_SYNC_ADOPTIONS_PER_SEC);
        assert!(rate < 30.0, "a 1ms gap must not dominate, rate was {rate}");

        let last = 7 * 50 + 1;
        let expected_ms = (1000.0 / rate).round() as u64;
        assert!(pace.is_catching_up_fast(t(last + expected_ms)));
        assert!(!pace.is_catching_up_fast(t(last + expected_ms + 1001)));
    }

    #[test]
    fn window_forgets_a_fast_regime_once_it_fills_with_slow_adoptions() {
        let mut pace = SyncAdoptionPace::default();
        for step in 0..8 {
            pace.record(t(step * 20), false);
        }
        assert!(pace.is_catching_up_fast(t(7 * 20)));

        // 200ms apart is 5/s. Eight of those replace the fast window.
        let origin = 1_000u64;
        for step in 0..8 {
            pace.record(t(origin + step * 200), false);
        }
        assert!(pace.rate_per_sec().is_some_and(|rate| rate < FAST_SYNC_ADOPTIONS_PER_SEC));
        assert!(!pace.is_catching_up_fast(t(origin + 7 * 200)));
    }

    #[test]
    fn simultaneous_adoptions_have_no_rate() {
        let mut pace = SyncAdoptionPace::default();
        for _ in 0..8 {
            pace.record(t(0), false);
        }
        assert_eq!(pace.rate_per_sec(), None);
        assert!(!pace.is_catching_up_fast(t(0)));
    }

    #[test]
    fn fast_sync_suppresses_until_the_next_adoption_is_overdue() {
        let mut pace = SyncAdoptionPace::default();
        pace.record(t(0), false);
        assert!(!pace.is_catching_up_fast(t(0)), "one adoption has no speed yet");

        // 50ms apart is 20/s.
        pace.record(t(50), false);
        assert!(pace.is_catching_up_fast(t(50)));
        assert!(pace.is_catching_up_fast(t(100)));
        assert!(!pace.is_catching_up_fast(t(1101)), "the next adoption should already have happened");
    }

    #[test]
    fn ten_per_second_is_not_fast_enough_to_suppress() {
        let mut pace = SyncAdoptionPace::default();
        pace.record(t(0), false);
        pace.record(t(100), false);
        assert!(!pace.is_catching_up_fast(t(100)));
    }
}
