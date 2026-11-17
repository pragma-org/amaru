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

//! Whether the adopted chain is caught up to the wall clock.
//!
//! `adopt_chain` stores the mode after each adoption. Other stages read it. A missing resource
//! reads as [`ConsensusMode::Sync`].

use std::time::Duration;

use amaru_kernel::{BlockHeight, EraHistory, HeaderHash, Slot};
use amaru_observability::info;
use amaru_pure_stage::{BoxFuture, ExternalEffectAPI, Instant, Resources, SendData};

/// Adopted tip is live when its slot onset is strictly within this of the wall clock.
pub const LIVE_TIP_LAG: Duration = Duration::from_secs(60);

/// How often to compare a near-now header with the adopted tip.
pub const CHAIN_LAG_LOG_INTERVAL: Duration = Duration::from_secs(60);

/// Compares near-now headers with the adopted tip, at most once per [`CHAIN_LAG_LOG_INTERVAL`],
/// to notice a node that has fallen behind. The caller runs the effects (tip, sync pace) and
/// logs; this keeps what the next check is judged against.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ChainLagMonitor {
    last_check: Option<Instant>,
    behind: Option<Behind>,
}

/// The adopted tip was behind the wall clock at the last check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Behind {
    /// `wall clock − tip slot onset`. Larger means further behind.
    pub lateness: Duration,
    pub prompted_by: PromptingHeader,
}

/// The near-now header that prompted a check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PromptingHeader {
    /// Not validated (yet). If it is rejected, or dropped with its connection, the height it
    /// claims says nothing about the chain, so it is never used.
    Unvalidated(HeaderHash),
    /// Passed validation at this height.
    Validated(BlockHeight),
}

/// Outcome of a check while the adopted tip is behind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChainLag {
    /// The first check while behind, the tip got closer, the tip reached the header of the
    /// previous check, or sync is still fast.
    Quiet,
    /// Behind a near-now header for a full interval and not catching up.
    Lagging,
}

/// Sync while catching up; live once the adopted tip is within [`LIVE_TIP_LAG`] of the wall clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ConsensusMode {
    Sync,
    Live,
}

impl ConsensusMode {
    pub fn is_live(self) -> bool {
        matches!(self, Self::Live)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sync => "sync",
            Self::Live => "live",
        }
    }
}

/// `Live` when `slot`'s onset is strictly within [`LIVE_TIP_LAG`] of `now`.
///
/// The adopted tip is a slot the node already holds, so the conversion does not apply the
/// forecast horizon. A slot that cannot be placed in the era history stays [`ConsensusMode::Sync`].
pub fn classify(slot: Slot, now: Instant, era: &EraHistory) -> ConsensusMode {
    let Ok(onset) = era.slot_to_relative_time_unchecked_horizon(slot) else {
        return ConsensusMode::Sync;
    };
    let elapsed = now.duration_since_global_epoch();
    if elapsed.abs_diff(onset) < LIVE_TIP_LAG { ConsensusMode::Live } else { ConsensusMode::Sync }
}

pub fn stored_mode(resources: &Resources) -> ConsensusMode {
    resources.get::<ConsensusMode>().map(|mode| *mode).unwrap_or(ConsensusMode::Sync)
}

pub fn is_live(resources: &Resources) -> bool {
    stored_mode(resources).is_live()
}

/// How far `slot`'s onset sits behind `now`. A tip slightly in the future has lateness zero.
/// `None` when the slot cannot be placed in the era history.
pub fn tip_lateness(slot: Slot, now: Instant, era: &EraHistory) -> Option<Duration> {
    let onset = era.slot_to_relative_time_unchecked_horizon(slot).ok()?;
    Some(now.duration_since_global_epoch().saturating_sub(onset))
}

impl ChainLagMonitor {
    /// Whether the last check is at least [`CHAIN_LAG_LOG_INTERVAL`] ago.
    pub fn is_due(&self, now: Instant) -> bool {
        self.last_check.is_none_or(|at| now.saturating_since(at) >= CHAIN_LAG_LOG_INTERVAL)
    }

    /// A near-now header is being checked.
    pub fn record_check(&mut self, now: Instant) {
        self.last_check = Some(now);
    }

    /// The adopted tip is within [`LIVE_TIP_LAG`] of the wall clock.
    pub fn caught_up(&mut self) {
        self.behind = None;
    }

    /// Judge a check that found the adopted tip `lateness` behind, prompted by the header `prompted_by`.
    ///
    /// `catching_up_fast` is the sync adoption pace: true while adoptions are still arriving faster
    /// than 10 per second and the next one is not overdue. The first check while behind is quiet,
    /// so the end of sync does not raise an error before the tip has had a minute to move.
    ///
    /// `our_height` is the block height of the adopted tip. The near-now header is seen before it
    /// can be adopted, so at the tip `lateness` is the time since the previous block. A tip that
    /// reached the height of the header behind the previous check has kept up, and what is left is
    /// a quiet stretch on chain. That height only counts once the header passed validation (see
    /// [`Self::header_validated`]); otherwise the previous check is judged on lateness alone.
    pub fn judge(
        &mut self,
        lateness: Duration,
        prompted_by: HeaderHash,
        our_height: BlockHeight,
        catching_up_fast: bool,
    ) -> ChainLag {
        let verdict = match self.behind {
            None => ChainLag::Quiet,
            Some(prev) if lateness < prev.lateness => ChainLag::Quiet,
            Some(Behind { prompted_by: PromptingHeader::Validated(height), .. }) if our_height >= height => {
                ChainLag::Quiet
            }
            Some(_) if catching_up_fast => ChainLag::Quiet,
            Some(_) => ChainLag::Lagging,
        };
        self.behind = Some(Behind { lateness, prompted_by: PromptingHeader::Unvalidated(prompted_by) });
        verdict
    }

    /// A header passed validation. If it prompted the last check, its height becomes what the next
    /// check is judged against.
    pub fn header_validated(&mut self, hash: HeaderHash, height: BlockHeight) {
        if let Some(behind) = self.behind.as_mut()
            && behind.prompted_by == PromptingHeader::Unvalidated(hash)
        {
            behind.prompted_by = PromptingHeader::Validated(height);
        }
    }

    /// State after a check at `last_check`, for tests.
    #[cfg(test)]
    pub fn for_tests(last_check: Instant, behind: Option<Behind>) -> Self {
        Self { last_check: Some(last_check), behind }
    }

    /// What the next check is judged against, for tests.
    #[cfg(test)]
    pub fn behind(&self) -> Option<Behind> {
        self.behind
    }
}

/// Recompute the mode from an adopted slot and store it. Returns the mode that was stored.
#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct UpdateConsensusModeEffect {
    pub slot: Slot,
    pub now: Instant,
}

impl ExternalEffectAPI for UpdateConsensusModeEffect {
    type Response = ConsensusMode;
    const SIMULATED_DURATION: amaru_pure_stage::DurationDist =
        crate::effect_timings::consensus_mode__UpdateConsensusModeEffect;

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let mode = {
            let era = resources.get::<EraHistory>();
            match era {
                Ok(era) => classify(self.slot, self.now, &era),
                Err(_) => ConsensusMode::Sync,
            }
        };
        let previous = stored_mode(&resources);
        if previous != mode {
            info!(consensus::tip::MODE, mode = mode.as_str(), previous = previous.as_str(), slot = self.slot);
            resources.put(mode);
        }
        self.wrap_sync(mode)
    }
}

/// Read the mode last stored by [`UpdateConsensusModeEffect`].
#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct QueryConsensusModeEffect;

impl ExternalEffectAPI for QueryConsensusModeEffect {
    type Response = ConsensusMode;

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync(stored_mode(&resources))
    }
}

#[cfg(test)]
mod tests {
    use amaru_observability::tracing::Level;
    use amaru_pure_stage::ExternalEffect;

    use super::*;
    use crate::stages::test_utils::{BufferWriter, install_test_log_capture};

    #[test]
    fn lag_under_sixty_seconds_is_live() {
        let era = EraHistory::default();
        let slot = Slot::from(100);
        let at_onset = Instant::at_offset(Duration::from_secs(100), Duration::ZERO);
        let within = Instant::at_offset(Duration::from_secs(159), Duration::ZERO);
        let at_limit = Instant::at_offset(Duration::from_secs(160), Duration::ZERO);
        let ahead = Instant::at_offset(Duration::from_secs(50), Duration::ZERO);
        assert_eq!(classify(slot, at_onset, &era), ConsensusMode::Live);
        assert_eq!(classify(slot, within, &era), ConsensusMode::Live);
        assert_eq!(classify(slot, ahead, &era), ConsensusMode::Live);
        assert_eq!(classify(slot, at_limit, &era), ConsensusMode::Sync);
    }

    #[test]
    fn mode_switch_is_logged_only_when_it_changes() {
        let logs = install_test_log_capture(BufferWriter::new());
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime");
        let resources = Resources::default();
        resources.put(EraHistory::default());
        resources.put(ConsensusMode::Sync);

        let run = |resources: Resources, now: Instant| {
            let effect = UpdateConsensusModeEffect { slot: Slot::from(100), now };
            rt.block_on(ExternalEffect::run(Box::new(effect), resources));
        };
        let at_tip = Instant::at_offset(Duration::from_secs(100), Duration::ZERO);
        let behind = Instant::at_offset(Duration::from_secs(200), Duration::ZERO);
        run(resources.clone(), at_tip);
        run(resources.clone(), at_tip);
        run(resources, behind);

        logs.logs()
            .assert_and_remove(Level::INFO, &["tip.mode", r#"mode="live""#, r#"previous="sync""#])
            .assert_and_remove(Level::INFO, &["tip.mode", r#"mode="sync""#, r#"previous="live""#])
            .assert_no_remaining_at([Level::INFO, Level::DEBUG, Level::WARN, Level::ERROR]);
    }

    fn at(secs: u64) -> Instant {
        Instant::at_offset(Duration::from_secs(secs), Duration::ZERO)
    }

    fn height(height: u64) -> BlockHeight {
        BlockHeight::from(height)
    }

    fn hash(byte: u8) -> HeaderHash {
        HeaderHash::from([byte; 32])
    }

    fn secs(secs: u64) -> Duration {
        Duration::from_secs(secs)
    }

    #[test]
    fn checks_are_due_once_per_interval() {
        let mut monitor = ChainLagMonitor::default();
        assert!(monitor.is_due(at(0)));
        monitor.record_check(at(0));
        assert!(!monitor.is_due(at(59)));
        assert!(monitor.is_due(at(60)));
    }

    #[test]
    fn first_check_behind_is_quiet_and_a_closer_tip_stays_quiet() {
        let mut monitor = ChainLagMonitor::default();
        assert_eq!(monitor.judge(secs(90), hash(1), height(5), false), ChainLag::Quiet);
        assert_eq!(
            monitor.behind(),
            Some(Behind { lateness: secs(90), prompted_by: PromptingHeader::Unvalidated(hash(1)) }),
            "the height is added once the header is validated"
        );
        assert_eq!(monitor.judge(secs(70), hash(2), height(9), false), ChainLag::Quiet);
        monitor.caught_up();
        assert_eq!(monitor.behind(), None);
    }

    #[test]
    fn unchanged_lateness_logs_unless_sync_is_still_fast() {
        let behind = Behind { lateness: secs(90), prompted_by: PromptingHeader::Validated(height(10)) };
        let mut monitor = ChainLagMonitor::for_tests(at(0), Some(behind));
        assert_eq!(monitor.judge(secs(90), hash(2), height(8), false), ChainLag::Lagging);
        let mut monitor = ChainLagMonitor::for_tests(at(0), Some(behind));
        assert_eq!(monitor.judge(secs(120), hash(2), height(6), true), ChainLag::Quiet);
    }

    #[test]
    fn a_tip_that_reached_the_previous_header_stays_quiet() {
        // Blocks 9, 10 and 11 at slots 10, 100 and 190: both headers arrive 90 seconds after their parent.
        let behind = Behind { lateness: secs(90), prompted_by: PromptingHeader::Validated(height(10)) };
        let mut monitor = ChainLagMonitor::for_tests(at(100), Some(behind));
        assert_eq!(monitor.judge(secs(90), hash(11), height(10), false), ChainLag::Quiet);
        // The same headers with block 10 still not adopted.
        let mut monitor = ChainLagMonitor::for_tests(at(100), Some(behind));
        assert_eq!(monitor.judge(secs(180), hash(11), height(9), false), ChainLag::Lagging);
    }

    #[test]
    fn an_unvalidated_header_is_judged_on_lateness() {
        // The header behind the previous check claimed height 2 and was rejected, or never validated.
        let behind = Behind { lateness: secs(90), prompted_by: PromptingHeader::Unvalidated(hash(1)) };
        let mut monitor = ChainLagMonitor::for_tests(at(0), Some(behind));
        assert_eq!(monitor.judge(secs(150), hash(2), height(10), false), ChainLag::Lagging);
    }

    #[test]
    fn only_the_header_behind_the_last_check_sets_the_height() {
        let mut monitor = ChainLagMonitor::default();
        monitor.judge(secs(90), hash(1), height(5), false);
        monitor.header_validated(hash(2), height(7));
        assert_eq!(monitor.behind().map(|b| b.prompted_by), Some(PromptingHeader::Unvalidated(hash(1))));
        monitor.header_validated(hash(1), height(6));
        assert_eq!(monitor.behind().map(|b| b.prompted_by), Some(PromptingHeader::Validated(height(6))));
    }
}
