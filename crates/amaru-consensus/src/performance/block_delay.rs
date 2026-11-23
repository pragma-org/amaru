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

use std::{collections::VecDeque, num::NonZeroUsize, time::Duration};

use amaru_kernel::MAINNET_GLOBAL_PARAMETERS;
use amaru_metrics::BlockDelayShares;

const MIN_BLOCKS_FOR_SHARES: usize = 45;

const CATCH_UP_DELAY: Duration = Duration::from_secs(20);

/// Slot-start-to-body delays of the most recently fetched blocks, oldest first.
#[derive(Debug, Clone)]
pub struct BlockDelayWindow {
    capacity: NonZeroUsize,
    delays: VecDeque<Duration>,
}

impl BlockDelayWindow {
    pub fn new(capacity: NonZeroUsize) -> Self {
        Self { capacity, delays: VecDeque::with_capacity(capacity.get()) }
    }

    /// Holds the last `k / 2` blocks, where `k` is `consensus_security_param`.
    pub fn for_security_param(consensus_security_param: u64) -> Self {
        let half = usize::try_from(consensus_security_param / 2).unwrap_or(usize::MAX);
        Self::new(NonZeroUsize::new(half).unwrap_or(NonZeroUsize::MIN))
    }

    /// Adds a block's delay, evicting the oldest once full.
    ///
    /// Until one block arrives within 20 seconds of its slot start, blocks are ignored as catch-up.
    /// After that, every block is counted. This is the rule cardano-node applies to its block-fetch
    /// client metrics.
    pub fn observe(&mut self, delay: Duration) -> BlockDelayObservation {
        if self.delays.is_empty() && delay > CATCH_UP_DELAY {
            return BlockDelayObservation::CatchingUp;
        }
        if self.delays.len() == self.capacity.get() {
            self.delays.pop_front();
        }
        self.delays.push_back(delay);
        let recent = (self.delays.len() >= MIN_BLOCKS_FOR_SHARES.min(self.capacity.get())).then(|| self.shares());
        BlockDelayObservation::Counted { recent }
    }

    fn shares(&self) -> BlockDelayShares {
        let within = |secs| self.delays.iter().filter(|delay| **delay <= Duration::from_secs(secs)).count() as u64;
        BlockDelayShares {
            blocks: self.delays.len() as u64,
            within_1s: within(1),
            within_3s: within(3),
            within_5s: within(5),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockDelayObservation {
    CatchingUp,
    /// `recent` is absent until the window holds enough blocks for the shares to mean something.
    Counted {
        recent: Option<BlockDelayShares>,
    },
}

impl Default for BlockDelayWindow {
    fn default() -> Self {
        Self::for_security_param(MAINNET_GLOBAL_PARAMETERS.consensus_security_param)
    }
}

#[cfg(test)]
mod tests {
    use test_case::test_case;

    use super::*;

    fn window(capacity: usize) -> BlockDelayWindow {
        BlockDelayWindow::new(NonZeroUsize::new(capacity).unwrap())
    }

    fn millis(ms: u64) -> Duration {
        Duration::from_millis(ms)
    }

    fn fill(window: &mut BlockDelayWindow, delays: &[Duration]) -> Option<BlockDelayObservation> {
        delays.iter().map(|delay| window.observe(*delay)).last()
    }

    fn counted(recent: Option<BlockDelayShares>) -> BlockDelayObservation {
        BlockDelayObservation::Counted { recent }
    }

    fn shares(blocks: u64, within_1s: u64, within_3s: u64, within_5s: u64) -> BlockDelayShares {
        BlockDelayShares { blocks, within_1s, within_3s, within_5s }
    }

    #[test_case(2160, 1080 ; "mainnet")]
    #[test_case(432, 216 ; "preview")]
    #[test_case(1, 1 ; "tiny k keeps one block")]
    #[test_case(0, 1 ; "zero k keeps one block")]
    fn capacity_is_half_the_security_param(k: u64, expected: usize) {
        assert_eq!(BlockDelayWindow::for_security_param(k).capacity.get(), expected);
    }

    #[test]
    fn no_shares_before_enough_blocks() {
        let mut window = window(1080);
        assert_eq!(fill(&mut window, &vec![millis(500); MIN_BLOCKS_FOR_SHARES - 1]), Some(counted(None)));
        assert_eq!(window.observe(millis(500)), counted(Some(shares(45, 45, 45, 45))));
    }

    #[test]
    fn small_window_reports_once_full() {
        let mut window = window(3);
        assert_eq!(fill(&mut window, &[millis(500), millis(500)]), Some(counted(None)));
        assert_eq!(window.observe(millis(500)), counted(Some(shares(3, 3, 3, 3))));
    }

    #[test_case(millis(999), shares(4, 4, 4, 4) ; "under one second")]
    #[test_case(millis(1_000), shares(4, 4, 4, 4) ; "exactly one second")]
    #[test_case(millis(1_001), shares(4, 3, 4, 4) ; "just over one second")]
    #[test_case(millis(3_001), shares(4, 3, 3, 4) ; "just over three seconds")]
    #[test_case(millis(5_000), shares(4, 3, 3, 4) ; "exactly five seconds")]
    #[test_case(millis(5_001), shares(4, 3, 3, 3) ; "just over five seconds")]
    fn shares_count_blocks_within_each_limit(last: Duration, expected: BlockDelayShares) {
        let mut window = window(4);
        assert_eq!(fill(&mut window, &[millis(100), millis(200), millis(300), last]), Some(counted(Some(expected))));
    }

    #[test]
    fn oldest_block_is_evicted_once_full() {
        let mut window = window(3);
        fill(&mut window, &[Duration::from_secs(10), millis(100), millis(100)]);
        assert_eq!(window.observe(millis(100)), counted(Some(shares(3, 3, 3, 3))));
    }

    #[test_case(&[], Duration::from_secs(21), BlockDelayObservation::CatchingUp ; "catch-up block before any on-time block")]
    #[test_case(&[], Duration::from_secs(20), counted(None) ; "first block at the catch-up limit")]
    #[test_case(&[millis(500)], Duration::from_secs(60), counted(None) ; "late block after an on-time block")]
    fn blocks_are_ignored_until_one_arrives_on_time(
        earlier: &[Duration],
        delay: Duration,
        expected: BlockDelayObservation,
    ) {
        let mut window = window(1080);
        fill(&mut window, earlier);
        assert_eq!(window.observe(delay), expected);
    }
}
