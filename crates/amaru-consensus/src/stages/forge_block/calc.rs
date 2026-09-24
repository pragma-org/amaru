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

//! Pure decisions used by [`super::stage`]. Kept free of effects so they can be
//! unit-tested without a simulation.

use std::time::{Duration, SystemTime};

use amaru_kernel::{BlockHeight, Epoch, KesEvolution, KesPeriodError, Point, Slot};
use amaru_pure_stage::Instant;

/// Forging starts this long before slot onset, so the block can diffuse as the slot begins.
/// A wake in the last `FORGE_LEAD_OFFSET` of a slot is too late to forge.
pub(super) const FORGE_LEAD_OFFSET: Duration = Duration::from_millis(50);

/// Parent of the block we are about to forge, given the adopted tip's slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ParentChoice {
    /// `tip.slot < lead_slot`: extend the adopted tip.
    AdoptedTip,
    /// `tip.slot == lead_slot`: a same-slot block is already adopted; extend its parent.
    AdoptedParent,
    /// `tip.slot > lead_slot`: forging would produce a block whose parent is later than its slot.
    MissedTipAhead,
}

/// Why a led slot was not forged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MissedSlotReason {
    OcertNotYetValid,
    OcertExpired,
    TipAhead,
    /// Woken for a slot the current schedule does not lead.
    NotLed,
    /// Woken too late to forge: in the last [`FORGE_LEAD_OFFSET`] of the slot, or after it.
    WokeLate,
}

impl MissedSlotReason {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::OcertNotYetValid => "ocert_not_yet_valid",
            Self::OcertExpired => "ocert_expired",
            Self::TipAhead => "tip_ahead",
            Self::NotLed => "not_led",
            Self::WokeLate => "woke_late",
        }
    }
}

/// Watch on the first header of a freeze window. The next epoch's schedule is
/// computed from this header's candidate nonce and is unsettled until `k` deep.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FreezeWatch {
    pub epoch: Epoch,
    pub point: Point,
}

impl FreezeWatch {
    /// Epoch whose leader schedule was computed from this freeze's candidate nonce.
    pub fn scheduled_epoch(self) -> Epoch {
        self.epoch + 1
    }
}

/// What to do with the freeze watch and next-epoch schedule after a new tip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FreezeDecision {
    pub freeze: Option<FreezeWatch>,
    /// Leader schedule to discard: the old watch's `scheduled_epoch`, if any.
    pub drop_epoch: Option<Epoch>,
    /// Leader schedule to compute: the tip's next epoch, if we just entered a freeze.
    pub schedule_epoch: Option<Epoch>,
}

/// Pick the parent of a block for `lead_slot` from the adopted tip's slot.
pub(super) fn choose_parent(tip_slot: Slot, lead_slot: Slot) -> ParentChoice {
    match tip_slot.cmp(&lead_slot) {
        std::cmp::Ordering::Less => ParentChoice::AdoptedTip,
        std::cmp::Ordering::Equal => ParentChoice::AdoptedParent,
        std::cmp::Ordering::Greater => ParentChoice::MissedTipAhead,
    }
}

/// Combine OCERT and parent checks into a miss reason, if any.
pub(super) fn missed_slot(
    coverage: Result<KesEvolution, KesPeriodError>,
    parent: ParentChoice,
) -> Option<MissedSlotReason> {
    match coverage {
        Err(KesPeriodError::StartsInTheFuture { .. }) => Some(MissedSlotReason::OcertNotYetValid),
        Err(KesPeriodError::Expired { .. }) => Some(MissedSlotReason::OcertExpired),
        Ok(_) => match parent {
            ParentChoice::MissedTipAhead => Some(MissedSlotReason::TipAhead),
            ParentChoice::AdoptedTip | ParentChoice::AdoptedParent => None,
        },
    }
}

/// Blocks adopted on the best chain since the freeze header.
pub(super) fn freeze_depth(freeze_height: BlockHeight, tip_height: BlockHeight) -> u64 {
    tip_height - freeze_height
}

pub(super) fn schedule_settled(blocks_since_freeze: u64, k: u64) -> bool {
    blocks_since_freeze >= k
}

/// Decide freeze-watch and next-epoch scheduling from the adopted tip.
///
/// - Enter the window of epoch `N` → watch this header, schedule `N+1`.
/// - Stay in that window (or past it but not `k` deep) → keep the watch.
/// - `k` blocks after the watch, and no longer in a freeze window → clear it so
///   a later epoch can enter.
/// - A later epoch's freeze window while a watch from an earlier epoch is still
///   set → replace the watch and schedule the new next epoch (do not drop the
///   schedule that just became current).
/// - Rollback past the watch (it is not an ancestor of the adopted tip) → drop
///   that watch's next-epoch schedule; re-enter if this tip is in a freeze window.
///   An adopted tip is never shorter than the previous one (`select_chain::cmp_tip`),
///   so a switch off the watch is an ancestor check, not a lower block height.
pub(super) fn decide_freeze(
    freeze: Option<&FreezeWatch>,
    tip: FreezeWatch,
    in_freeze_window: bool,
    freeze_is_ancestor: bool,
    k: u64,
) -> FreezeDecision {
    let rolled_back = freeze.is_some_and(|_| !freeze_is_ancestor);
    let drop_epoch = freeze.filter(|_| rolled_back).map(|watch| watch.scheduled_epoch());

    if rolled_back {
        return if in_freeze_window {
            FreezeDecision { freeze: Some(tip), drop_epoch, schedule_epoch: Some(tip.scheduled_epoch()) }
        } else {
            FreezeDecision { freeze: None, drop_epoch, schedule_epoch: None }
        };
    }

    if in_freeze_window {
        return match freeze {
            Some(watch) if watch.epoch == tip.epoch => {
                FreezeDecision { freeze: Some(*watch), drop_epoch: None, schedule_epoch: None }
            }
            _ => FreezeDecision { freeze: Some(tip), drop_epoch: None, schedule_epoch: Some(tip.scheduled_epoch()) },
        };
    }

    match freeze {
        Some(watch) if schedule_settled(freeze_depth(watch.point.block_height(), tip.point.block_height()), k) => {
            FreezeDecision { freeze: None, drop_epoch: None, schedule_epoch: None }
        }
        Some(watch) => FreezeDecision { freeze: Some(*watch), drop_epoch: None, schedule_epoch: None },
        None => FreezeDecision { freeze: None, drop_epoch: None, schedule_epoch: None },
    }
}

/// Where `now` sits relative to the interval in which forging may start: from
/// [`FORGE_LEAD_OFFSET`] before `onset` until [`FORGE_LEAD_OFFSET`] before `end`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ForgeWindow {
    /// Earlier than the window. Wait this long before forging.
    Wait(Duration),
    Open,
    /// In the last [`FORGE_LEAD_OFFSET`] of the slot, or later.
    Late,
}

pub(super) fn forge_window(now: Instant, onset: Instant, end: Instant) -> ForgeWindow {
    let open_at = onset - FORGE_LEAD_OFFSET;
    let close_at = end - FORGE_LEAD_OFFSET;
    if now < open_at {
        ForgeWindow::Wait(open_at.checked_since(now).unwrap_or(Duration::ZERO))
    } else if now < close_at {
        ForgeWindow::Open
    } else {
        ForgeWindow::Late
    }
}

/// Instant at which `DueLead` should fire: `offset` before onset, but not in the past.
pub(super) fn lead_fire_at(onset: Instant, now: Instant, offset: Duration) -> Instant {
    let early = onset - offset;
    if early > now { early } else { now }
}

/// Remaining time until slot onset, if we finished forging early. `None` means publish now.
pub(super) fn wait_until_onset(onset: Instant, now: Instant) -> Option<Duration> {
    onset.checked_since(now).filter(|duration| *duration > Duration::ZERO)
}

/// Map a duration since Cardano system start onto the simulation clock.
pub(super) fn instant_for_relative(now: Instant, relative: Duration) -> Instant {
    let elapsed = now.duration_since_global_epoch();
    if relative >= elapsed { now + (relative - elapsed) } else { now - (elapsed - relative) }
}

/// UTC civil time, `YYYY-MM-DDTHH:MM:SS.ffffffZ`, matching the log envelope's timestamp.
pub(super) fn format_utc_timestamp(time: SystemTime) -> Option<String> {
    let duration = time.duration_since(SystemTime::UNIX_EPOCH).ok()?;
    let secs = duration.as_secs();
    let days = secs / 86_400;
    let tod = secs % 86_400;
    let hours = tod / 3_600;
    let minutes = (tod % 3_600) / 60;
    let seconds = tod % 60;
    let (year, month, day) = days_to_ymd(days);
    Some(format!("{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}Z"))
}

fn days_to_ymd(days: u64) -> (u64, u64, u64) {
    let mut year = 1970u64;
    let mut remaining = days;
    loop {
        let days_in_year = if is_leap_year(year) { 366 } else { 365 };
        if remaining < days_in_year {
            break;
        }
        remaining -= days_in_year;
        year += 1;
    }
    let month_lengths: [u64; 12] = if is_leap_year(year) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    let mut month = 0u64;
    for days_in_month in month_lengths {
        if remaining < days_in_month {
            break;
        }
        remaining -= days_in_month;
        month += 1;
    }
    (year, month + 1, remaining + 1)
}

fn is_leap_year(year: u64) -> bool {
    (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400)
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use amaru_kernel::{BlockHeight, Epoch, HeaderHash, KesPeriod};

    use super::*;

    fn slot(n: u64) -> Slot {
        Slot::from(n)
    }

    fn instant(secs: u64) -> Instant {
        Instant::at_offset(Duration::from_secs(secs), Duration::ZERO)
    }

    #[test]
    fn choose_parent_extends_earlier_tip() {
        assert_eq!(choose_parent(slot(10), slot(11)), ParentChoice::AdoptedTip);
    }

    #[test]
    fn choose_parent_uses_parent_on_same_slot() {
        assert_eq!(choose_parent(slot(11), slot(11)), ParentChoice::AdoptedParent);
    }

    #[test]
    fn choose_parent_misses_when_tip_is_ahead() {
        assert_eq!(choose_parent(slot(12), slot(11)), ParentChoice::MissedTipAhead);
    }

    #[test]
    fn missed_slot_prefers_ocert_over_tip() {
        let period = KesPeriod::from;
        let expired = period(67).evolutions_since(period(5), 62);
        let covered = period(66).evolutions_since(period(5), 62);
        let early = period(4).evolutions_since(period(5), 62);
        assert_eq!(missed_slot(expired, ParentChoice::MissedTipAhead), Some(MissedSlotReason::OcertExpired));
        assert_eq!(missed_slot(early, ParentChoice::AdoptedTip), Some(MissedSlotReason::OcertNotYetValid));
        assert_eq!(missed_slot(covered.clone(), ParentChoice::MissedTipAhead), Some(MissedSlotReason::TipAhead));
        assert_eq!(missed_slot(covered, ParentChoice::AdoptedParent), None);
    }

    #[test]
    fn freeze_depth_is_height_difference() {
        assert_eq!(freeze_depth(BlockHeight::from(10), BlockHeight::from(15)), 5);
        assert_eq!(freeze_depth(BlockHeight::from(15), BlockHeight::from(10)), 0);
    }

    #[test]
    fn schedule_settles_at_k() {
        assert!(!schedule_settled(2159, 2160));
        assert!(schedule_settled(2160, 2160));
        assert!(schedule_settled(2161, 2160));
    }

    fn watch(epoch: u64, slot: u64, height: u64, tag: u8) -> FreezeWatch {
        FreezeWatch {
            epoch: Epoch::from(epoch),
            point: Point::Specific(Slot::from(slot), HeaderHash::from([tag; 32]), BlockHeight::from(height)),
        }
    }

    fn enter(tip: FreezeWatch) -> FreezeDecision {
        FreezeDecision { freeze: Some(tip), drop_epoch: None, schedule_epoch: Some(tip.scheduled_epoch()) }
    }

    fn hold(watch: FreezeWatch) -> FreezeDecision {
        FreezeDecision { freeze: Some(watch), drop_epoch: None, schedule_epoch: None }
    }

    fn clear() -> FreezeDecision {
        FreezeDecision { freeze: None, drop_epoch: None, schedule_epoch: None }
    }

    #[test]
    fn freeze_enters_on_first_tip_in_the_window() {
        let tip = watch(5, 80, 10, 1);
        assert_eq!(decide_freeze(None, tip, true, true, 3), enter(tip));
    }

    #[test]
    fn freeze_holds_on_later_tips_in_the_same_window() {
        let first = watch(5, 80, 10, 1);
        let later = watch(5, 90, 12, 2);
        assert_eq!(decide_freeze(Some(&first), later, true, true, 3), hold(first));
    }

    #[test]
    fn freeze_holds_across_the_epoch_boundary_until_k_deep() {
        let first = watch(5, 80, 10, 1);
        let next_epoch = watch(6, 100, 11, 2);
        assert_eq!(decide_freeze(Some(&first), next_epoch, false, true, 3), hold(first));
    }

    #[test]
    fn freeze_clears_once_k_deep_and_out_of_the_window() {
        let first = watch(5, 80, 10, 1);
        let settled = watch(6, 100, 13, 2);
        assert_eq!(decide_freeze(Some(&first), settled, false, true, 3), clear());
    }

    #[test]
    fn freeze_enters_the_next_epoch_window_after_clearing() {
        let tip = watch(6, 180, 20, 3);
        assert_eq!(decide_freeze(None, tip, true, true, 3), enter(tip));
    }

    #[test]
    fn freeze_replaces_the_watch_when_a_later_epoch_window_opens() {
        let old = watch(5, 80, 10, 1);
        let next_window = watch(6, 180, 40, 3);
        assert_eq!(decide_freeze(Some(&old), next_window, true, true, 3), enter(next_window));
    }

    #[test]
    fn freeze_resets_on_a_fork_that_drops_the_watch() {
        let first = watch(5, 80, 10, 1);
        let fork = watch(5, 90, 12, 9);
        assert_eq!(
            decide_freeze(Some(&first), fork, true, false, 3),
            FreezeDecision {
                freeze: Some(fork),
                drop_epoch: Some(first.scheduled_epoch()),
                schedule_epoch: Some(fork.scheduled_epoch()),
            }
        );
    }

    #[test]
    fn freeze_exits_on_a_chain_switch_out_of_the_window() {
        let first = watch(5, 80, 10, 1);
        // Denser fork: greater height, earlier slot, freeze header not an ancestor.
        let switched = watch(5, 50, 12, 8);
        assert_eq!(
            decide_freeze(Some(&first), switched, false, false, 3),
            FreezeDecision { freeze: None, drop_epoch: Some(first.scheduled_epoch()), schedule_epoch: None }
        );
    }

    #[test]
    fn forge_window_opens_at_the_lead_offset_and_closes_one_offset_before_the_end() {
        let onset = instant(10);
        let end = instant(11);
        assert_eq!(
            forge_window(onset - FORGE_LEAD_OFFSET - Duration::from_millis(30), onset, end),
            ForgeWindow::Wait(Duration::from_millis(30))
        );
        assert_eq!(forge_window(onset - FORGE_LEAD_OFFSET, onset, end), ForgeWindow::Open);
        assert_eq!(forge_window(onset, onset, end), ForgeWindow::Open);
        assert_eq!(forge_window(end - FORGE_LEAD_OFFSET - Duration::from_millis(1), onset, end), ForgeWindow::Open);
        assert_eq!(forge_window(end - FORGE_LEAD_OFFSET, onset, end), ForgeWindow::Late);
        assert_eq!(forge_window(end, onset, end), ForgeWindow::Late);
    }

    #[test]
    fn lead_fire_at_is_offset_before_onset_unless_that_is_past() {
        let onset = instant(10);
        assert_eq!(lead_fire_at(onset, instant(1), Duration::from_secs(2)), instant(8));
        assert_eq!(lead_fire_at(onset, instant(9), Duration::from_secs(2)), instant(9));
    }

    #[test]
    fn wait_until_onset_is_none_when_already_at_or_past_onset() {
        assert_eq!(wait_until_onset(instant(10), instant(8)), Some(Duration::from_secs(2)));
        assert_eq!(wait_until_onset(instant(10), instant(10)), None);
        assert_eq!(wait_until_onset(instant(10), instant(11)), None);
    }

    #[test]
    fn utc_timestamp_matches_known_ouroboros_starts() {
        assert_eq!(format_utc_timestamp(SystemTime::UNIX_EPOCH).unwrap(), "1970-01-01T00:00:00Z");
        let mainnet = SystemTime::UNIX_EPOCH + Duration::from_millis(1_506_203_091_000);
        assert_eq!(format_utc_timestamp(mainnet).unwrap(), "2017-09-23T21:44:51Z");
        let preprod = SystemTime::UNIX_EPOCH + Duration::from_millis(1_654_041_600_000);
        assert_eq!(format_utc_timestamp(preprod).unwrap(), "2022-06-01T00:00:00Z");
        let with_fraction = SystemTime::UNIX_EPOCH + Duration::from_micros(1_500_000);
        assert_eq!(format_utc_timestamp(with_fraction).unwrap(), "1970-01-01T00:00:01Z");
    }

    #[test]
    fn instant_for_relative_is_symmetric_with_duration_since_global_epoch() {
        let now = instant(10);
        let at = instant_for_relative(now, Duration::from_secs(15));
        assert_eq!(at.duration_since_global_epoch(), Duration::from_secs(15));
        let past = instant_for_relative(now, Duration::from_secs(4));
        assert_eq!(past.duration_since_global_epoch(), Duration::from_secs(4));
    }
}
