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

use amaru_kernel::{BlockHeight, EraHistory, Slot};
use amaru_observability::info;
use amaru_pure_stage::{BoxFuture, ExternalEffectAPI, Instant, Resources, SendData};

/// Adopted tip is live when its slot onset is strictly within this of the wall clock.
pub const LIVE_TIP_LAG: Duration = Duration::from_secs(60);

/// How often to compare a near-now header with the adopted tip.
pub const CHAIN_LAG_LOG_INTERVAL: Duration = Duration::from_secs(60);

/// One observation that the adopted tip was behind the wall clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ChainLagSample {
    pub at: Instant,
    /// `wall clock − tip slot onset`. Larger means further behind.
    pub lateness: Duration,
    /// Block height of the near-now header that prompted the check, once that header passed
    /// validation. A height claimed by a header that was rejected or never validated says nothing
    /// about the chain.
    pub live_height: Option<BlockHeight>,
}

/// What to do with a near-now header once the previous sample, if any, is old enough to judge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChainLagWatch {
    /// Adopted tip is within [`LIVE_TIP_LAG`].
    CaughtUp,
    /// Behind, and either this is the first sample, the tip got closer, the tip reached the
    /// header of the previous sample, or sync is still fast.
    Quiet(ChainLagSample),
    /// Behind a near-now header for a full interval and not catching up.
    Lagging(ChainLagSample),
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

/// Decide whether a near-now header means the node has fallen behind.
///
/// The caller invokes this at most once per [`CHAIN_LAG_LOG_INTERVAL`]. `catching_up_fast` is the
/// sync adoption pace: true while adoptions are still arriving faster than 10 per second and the
/// next one is not overdue. The first sample while behind is quiet, so the end of sync does not
/// raise an error before the tip has had a minute to move.
///
/// `our_height` is the block height of the adopted tip. The near-now header is seen before it can
/// be adopted, so at the tip `lateness` is the time since the previous block. A tip that reached
/// the height of the previous sample's header has kept up, and what is left is a quiet stretch on
/// chain. The returned sample has no height: the caller adds it once the header is validated. A
/// previous sample without a height is judged on lateness alone.
pub fn judge_chain_lag(
    previous: Option<ChainLagSample>,
    now: Instant,
    lateness: Duration,
    our_height: BlockHeight,
    catching_up_fast: bool,
) -> ChainLagWatch {
    if lateness < LIVE_TIP_LAG {
        return ChainLagWatch::CaughtUp;
    }
    let sample = ChainLagSample { at: now, lateness, live_height: None };
    match previous {
        None => ChainLagWatch::Quiet(sample),
        Some(prev) if lateness < prev.lateness => ChainLagWatch::Quiet(sample),
        Some(prev) if prev.live_height.is_some_and(|height| our_height >= height) => ChainLagWatch::Quiet(sample),
        Some(_) if catching_up_fast => ChainLagWatch::Quiet(sample),
        Some(_) => ChainLagWatch::Lagging(sample),
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

    #[test]
    fn first_behind_sample_is_quiet_and_a_closer_tip_stays_quiet() {
        let first = judge_chain_lag(None, at(0), Duration::from_secs(90), height(5), false);
        let ChainLagWatch::Quiet(sample) = first else { panic!("first sample must not log") };
        assert_eq!(sample.live_height, None, "the height is added once the header is validated");
        let closer = judge_chain_lag(Some(sample), at(60), Duration::from_secs(70), height(9), false);
        assert!(matches!(closer, ChainLagWatch::Quiet(_)));
        assert!(matches!(
            judge_chain_lag(Some(sample), at(60), Duration::from_secs(30), height(11), false),
            ChainLagWatch::CaughtUp
        ));
    }

    #[test]
    fn unchanged_lateness_logs_unless_sync_is_still_fast() {
        let sample = ChainLagSample { at: at(0), lateness: Duration::from_secs(90), live_height: Some(height(10)) };
        assert!(matches!(
            judge_chain_lag(Some(sample), at(60), Duration::from_secs(90), height(8), false),
            ChainLagWatch::Lagging(_)
        ));
        assert!(matches!(
            judge_chain_lag(Some(sample), at(60), Duration::from_secs(120), height(6), true),
            ChainLagWatch::Quiet(_)
        ));
    }

    #[test]
    fn a_tip_that_reached_the_previous_header_stays_quiet() {
        // Blocks 9, 10 and 11 at slots 10, 100 and 190: both headers arrive 90 seconds after their parent.
        let sample = ChainLagSample { at: at(100), lateness: Duration::from_secs(90), live_height: Some(height(10)) };
        let kept_up = judge_chain_lag(Some(sample), at(190), Duration::from_secs(90), height(10), false);
        assert_eq!(
            kept_up,
            ChainLagWatch::Quiet(ChainLagSample { at: at(190), lateness: Duration::from_secs(90), live_height: None })
        );
        // The same headers with block 10 still not adopted.
        assert!(matches!(
            judge_chain_lag(Some(sample), at(190), Duration::from_secs(180), height(9), false),
            ChainLagWatch::Lagging(_)
        ));
    }

    #[test]
    fn a_sample_without_height_is_judged_on_lateness() {
        // The header that prompted the sample claimed height 2 and was rejected.
        let sample = ChainLagSample { at: at(0), lateness: Duration::from_secs(90), live_height: None };
        assert!(matches!(
            judge_chain_lag(Some(sample), at(60), Duration::from_secs(150), height(10), false),
            ChainLagWatch::Lagging(_)
        ));
    }
}
