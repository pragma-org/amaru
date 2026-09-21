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

use std::time::Duration;

use amaru_kernel::{BlockHeight, Epoch, Point, Slot};
use amaru_pure_stage::Instant;

/// How far before slot onset `LeadSlot` is armed so forging can finish in time.
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

/// Whether the operational certificate covers a KES period.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OcertCoverage {
    Valid,
    NotYetValid,
    Expired,
}

/// Why a led slot was not forged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MissedSlotReason {
    OcertNotYetValid,
    OcertExpired,
    TipAhead,
}

impl MissedSlotReason {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::OcertNotYetValid => "ocert_not_yet_valid",
            Self::OcertExpired => "ocert_expired",
            Self::TipAhead => "tip_ahead",
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

/// OCERT window: `start_period <= kes_period < start_period + max_evolutions`.
///
/// Mainnet `max_evolutions` is 62 (Sum6KES has 64 periods, two of margin).
pub(super) fn ocert_covers(kes_period: u64, start_period: u64, max_evolutions: u64) -> OcertCoverage {
    if kes_period < start_period {
        OcertCoverage::NotYetValid
    } else if kes_period >= start_period.saturating_add(max_evolutions) {
        OcertCoverage::Expired
    } else {
        OcertCoverage::Valid
    }
}

/// Combine OCERT and parent checks into a miss reason, if any.
pub(super) fn missed_slot(coverage: OcertCoverage, parent: ParentChoice) -> Option<MissedSlotReason> {
    match coverage {
        OcertCoverage::NotYetValid => Some(MissedSlotReason::OcertNotYetValid),
        OcertCoverage::Expired => Some(MissedSlotReason::OcertExpired),
        OcertCoverage::Valid => match parent {
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

/// Drop led slots whose onset is no longer in the future. Order is preserved.
pub(super) fn drop_past_led_slots(led: &mut Vec<Slot>, now: Instant, slot_onset: impl Fn(Slot) -> Instant) {
    led.retain(|&slot| slot_onset(slot) > now);
}

/// Next led slot that can still be published (`onset > now`).
pub(super) fn next_schedulable_slot(
    led: &[Slot],
    now: Instant,
    slot_onset: impl Fn(Slot) -> Option<Instant>,
) -> Option<(Slot, Instant)> {
    led.iter().copied().find_map(|slot| {
        let onset = slot_onset(slot)?;
        (onset > now).then_some((slot, onset))
    })
}

/// Replace led slots that fall in `[from, until)` with `new`.
pub(super) fn replace_epoch_slots(led: &mut Vec<Slot>, from: Slot, until: Slot, new: impl IntoIterator<Item = Slot>) {
    led.retain(|slot| *slot < from || *slot >= until);
    led.extend(new);
    led.sort();
    led.dedup();
}

/// Instant at which `LeadSlot` should fire: `offset` before onset, but not in the past.
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

#[cfg(test)]
mod tests {
    use amaru_kernel::{BlockHeight, Epoch, HeaderHash};

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
    fn ocert_covers_the_inclusive_start_and_excludes_the_end() {
        assert_eq!(ocert_covers(5, 5, 62), OcertCoverage::Valid);
        assert_eq!(ocert_covers(66, 5, 62), OcertCoverage::Valid);
        assert_eq!(ocert_covers(67, 5, 62), OcertCoverage::Expired);
        assert_eq!(ocert_covers(4, 5, 62), OcertCoverage::NotYetValid);
    }

    #[test]
    fn missed_slot_prefers_ocert_over_tip() {
        assert_eq!(
            missed_slot(OcertCoverage::Expired, ParentChoice::MissedTipAhead),
            Some(MissedSlotReason::OcertExpired)
        );
        assert_eq!(missed_slot(OcertCoverage::Valid, ParentChoice::MissedTipAhead), Some(MissedSlotReason::TipAhead));
        assert_eq!(missed_slot(OcertCoverage::Valid, ParentChoice::AdoptedParent), None);
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
    fn unpublished_slots_are_those_whose_onset_is_still_ahead() {
        let onset = |s: Slot| instant(u64::from(s));
        let mut led = vec![slot(5), slot(8), slot(3)];
        drop_past_led_slots(&mut led, instant(5), onset);
        assert_eq!(led, vec![slot(8)]);
        let mut led = vec![slot(5)];
        drop_past_led_slots(&mut led, instant(4), onset);
        assert_eq!(led, vec![slot(5)]);
    }

    #[test]
    fn next_schedulable_slot_skips_past_onsets() {
        let onset = |s: Slot| Some(instant(u64::from(s)));
        assert_eq!(next_schedulable_slot(&[slot(3), slot(5), slot(8)], instant(5), onset), Some((slot(8), instant(8))));
        assert_eq!(next_schedulable_slot(&[slot(8)], instant(4), onset), Some((slot(8), instant(8))));
    }

    #[test]
    fn replace_epoch_slots_swaps_a_range() {
        let mut led = vec![slot(1), slot(5), slot(9)];
        replace_epoch_slots(&mut led, slot(4), slot(8), [slot(6), slot(7)]);
        assert_eq!(led, vec![slot(1), slot(6), slot(7), slot(9)]);
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
    fn instant_for_relative_is_symmetric_with_duration_since_global_epoch() {
        let now = instant(10);
        let at = instant_for_relative(now, Duration::from_secs(15));
        assert_eq!(at.duration_since_global_epoch(), Duration::from_secs(15));
        let past = instant_for_relative(now, Duration::from_secs(4));
        assert_eq!(past.duration_since_global_epoch(), Duration::from_secs(4));
    }
}
