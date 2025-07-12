// Copyright 2024 PRAGMA
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

#![feature(step_trait)]

use minicbor::{Decode, Decoder, Encode};
use serde::{Deserialize, Serialize};
use std::{
    fmt::Display,
    iter::Step,
    ops::{Add, Sub},
    str::FromStr,
};

#[cfg(any(test, feature = "test-utils"))]
use proptest::prelude::{Arbitrary, BoxedStrategy, Strategy};

#[derive(Clone, Debug, Copy, PartialEq, PartialOrd, Ord, Eq, Serialize, Deserialize, Default)]
#[repr(transparent)]
pub struct Slot(u64);

impl Display for Slot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, PartialEq, Eq, thiserror::Error, serde::Serialize, serde::Deserialize)]
pub enum SlotArithmeticError {
    #[error("slot arithmetic overflow, substracting {1} from {0}")]
    Underflow(u64, u64),
}

impl Slot {
    fn elapsed_from(&self, slot: Slot) -> Result<u64, SlotArithmeticError> {
        if self.0 < slot.0 {
            return Err(SlotArithmeticError::Underflow(self.0, slot.0));
        }
        Ok(self.0 - slot.0)
    }

    fn offset_by(&self, slots_elapsed: u64) -> Slot {
        Slot(self.0 + slots_elapsed)
    }
}

impl From<u64> for Slot {
    fn from(slot: u64) -> Slot {
        Slot(slot)
    }
}

impl From<Slot> for u64 {
    fn from(slot: Slot) -> u64 {
        slot.0
    }
}

impl<C> Encode<C> for Slot {
    fn encode<W: minicbor::encode::Write>(
        &self,
        e: &mut minicbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), minicbor::encode::Error<W::Error>> {
        self.0.encode(e, ctx)
    }
}

impl<'b, C> Decode<'b, C> for Slot {
    fn decode(d: &mut Decoder<'b>, _ctx: &mut C) -> Result<Self, minicbor::decode::Error> {
        d.u64().map(Slot)
    }
}

#[derive(Clone, Debug, Copy, PartialEq, PartialOrd, Ord, Eq, Serialize, Deserialize, Default)]
#[repr(transparent)]
pub struct Epoch(u64);

#[cfg(any(test, feature = "test-utils"))]
impl Arbitrary for Epoch {
    type Parameters = ();
    type Strategy = BoxedStrategy<Self>;

    fn arbitrary_with(_: Self::Parameters) -> Self::Strategy {
        (0..u64::MAX).prop_map(Epoch::from).boxed()
    }
}

impl Display for Epoch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for Epoch {
    type Err = std::num::ParseIntError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse::<u64>().map(Epoch)
    }
}

impl From<u64> for Epoch {
    fn from(epoch: u64) -> Epoch {
        Epoch(epoch)
    }
}

impl From<Epoch> for u64 {
    fn from(epoch: Epoch) -> u64 {
        epoch.0
    }
}

impl<C> Encode<C> for Epoch {
    fn encode<W: minicbor::encode::Write>(
        &self,
        e: &mut minicbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), minicbor::encode::Error<W::Error>> {
        self.0.encode(e, ctx)
    }
}

impl<'b, C> Decode<'b, C> for Epoch {
    fn decode(d: &mut Decoder<'b>, _ctx: &mut C) -> Result<Self, minicbor::decode::Error> {
        d.u64().map(Epoch)
    }
}

impl Add<u64> for Epoch {
    type Output = Self;

    fn add(self, rhs: u64) -> Self::Output {
        Epoch(self.0 + rhs)
    }
}

impl Sub<u64> for Epoch {
    type Output = Self;

    fn sub(self, rhs: u64) -> Self::Output {
        Epoch(self.0 - rhs)
    }
}

impl Sub<Epoch> for Epoch {
    type Output = u64;

    fn sub(self, rhs: Epoch) -> Self::Output {
        self.0 - rhs.0
    }
}

impl Step for Epoch {
    fn steps_between(start: &Self, end: &Self) -> (usize, Option<usize>) {
        u64::steps_between(&start.0, &end.0)
    }

    fn forward_checked(start: Self, count: usize) -> Option<Self> {
        u64::forward_checked(start.0, count).map(Epoch)
    }

    fn backward_checked(start: Self, count: usize) -> Option<Self> {
        u64::backward_checked(start.0, count).map(Epoch)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bound {
    pub time_ms: u64, // Milliseconds
    pub slot: Slot,
    pub epoch: Epoch,
}

#[allow(dead_code)]
impl Bound {
    fn genesis() -> Bound {
        Bound {
            time_ms: 0,
            slot: Slot(0),
            epoch: Epoch(0),
        }
    }
}

impl<C> Encode<C> for Bound {
    fn encode<W: minicbor::encode::Write>(
        &self,
        e: &mut minicbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), minicbor::encode::Error<W::Error>> {
        e.begin_array()?;
        self.time_ms.encode(e, ctx)?;
        self.slot.encode(e, ctx)?;
        self.epoch.encode(e, ctx)?;
        e.end()?;
        Ok(())
    }
}

impl<'b, C> Decode<'b, C> for Bound {
    fn decode(d: &mut Decoder<'b>, ctx: &mut C) -> Result<Self, minicbor::decode::Error> {
        let _ = d.array()?;
        let time_ms = d.u64()?;
        let slot = d.decode()?;
        let epoch = d.decode_with(ctx)?;
        d.skip()?;
        Ok(Bound {
            time_ms,
            slot,
            epoch,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EraParams {
    pub epoch_size_slots: u64,
    pub slot_length: u64, // Milliseconds
}

impl EraParams {
    pub fn new(epoch_size_slots: u64, slot_length: u64) -> Option<Self> {
        if epoch_size_slots == 0 {
            return None;
        }
        if slot_length == 0 {
            return None;
        }
        Some(EraParams {
            epoch_size_slots,
            slot_length,
        })
    }
}

impl<C> Encode<C> for EraParams {
    fn encode<W: minicbor::encode::Write>(
        &self,
        e: &mut minicbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), minicbor::encode::Error<W::Error>> {
        e.begin_array()?;
        self.epoch_size_slots.encode(e, ctx)?;
        self.slot_length.encode(e, ctx)?;
        e.end()?;
        Ok(())
    }
}

impl<'b, C> Decode<'b, C> for EraParams {
    fn decode(d: &mut Decoder<'b>, _ctx: &mut C) -> Result<Self, minicbor::decode::Error> {
        let _ = d.array()?;
        let epoch_size_slots = d.decode()?;
        let slot_length = d.decode()?;
        d.skip()?;
        Ok(EraParams {
            epoch_size_slots,
            slot_length,
        })
    }
}

// The start is inclusive and the end is exclusive. In a valid EraHistory, the
// end of each era will equal the start of the next one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Summary {
    pub start: Bound,
    pub end: Option<Bound>,
    pub params: EraParams,
}

impl Summary {
    /// Checks whether the current `Summary` ends after the given slot; In case
    /// where the Summary doesn't have any upper bound, then we check whether the
    /// point is within a foreseeable horizon.
    pub fn contains_slot(&self, slot: &Slot, tip: &Slot, stability_window: &Slot) -> bool {
        &self
            .end
            .as_ref()
            .map(|end| end.slot)
            .unwrap_or_else(|| self.calculate_end_bound(tip, stability_window).slot)
            >= slot
    }

    /// Like contains_slot, but doesn't enforce anything about the upper bound. So when there's no
    /// upper bound, the slot is simply always considered within the era.
    pub fn contains_slot_unchecked_horizon(&self, slot: &Slot) -> bool {
        self.end
            .as_ref()
            .map(|end| &end.slot >= slot)
            .unwrap_or(true)
    }

    pub fn contains_epoch(&self, epoch: &Epoch, tip: &Slot, stability_window: &Slot) -> bool {
        &self
            .end
            .as_ref()
            .map(|end| end.epoch)
            .unwrap_or_else(|| self.calculate_end_bound(tip, stability_window).epoch)
            > epoch
    }

    /// Like contains_epoch, but doesn't enforce anything about the upper bound. So when there's no
    /// upper bound, the epoch is simply always considered within the era.
    pub fn contains_epoch_unchecked_horizon(&self, epoch: &Epoch) -> bool {
        self.end
            .as_ref()
            .map(|end| &end.epoch > epoch)
            .unwrap_or(true)
    }

    /// Calculate a virtual end `Bound` given a time and the last era summary that we know of.
    ///
    /// **pre-condition**: the provided tip must be after (or equal) to the start of this era.
    fn calculate_end_bound(&self, tip: &Slot, stability_window: &Slot) -> Bound {
        let Self { start, params, end } = self;

        debug_assert!(end.is_none());

        // NOTE: The +1 here is justified by the fact that upper bound in era summaries are
        // exclusive. So if our tip is *exactly* at the frontier of the stability area, then
        // technically, we already can foresee time in the next epoch.
        let end_of_stable_window = start.slot.0.max(tip.0 + 1) + stability_window.0;

        let delta_slots = end_of_stable_window - start.slot.0;

        let delta_epochs = delta_slots / params.epoch_size_slots
            + if delta_slots % params.epoch_size_slots == 0 {
                0
            } else {
                1
            };

        let max_foreseeable_epoch = start.epoch.0 + delta_epochs;

        let foreseeable_slots = delta_epochs * params.epoch_size_slots;

        Bound {
            time_ms: start.time_ms + foreseeable_slots * params.slot_length,
            slot: Slot(start.slot.0 + foreseeable_slots),
            epoch: Epoch(max_foreseeable_epoch),
        }
    }
}

impl<C> Encode<C> for Summary {
    fn encode<W: minicbor::encode::Write>(
        &self,
        e: &mut minicbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), minicbor::encode::Error<W::Error>> {
        e.begin_array()?;
        self.start.encode(e, ctx)?;
        self.end.encode(e, ctx)?;
        self.params.encode(e, ctx)?;
        e.end()?;
        Ok(())
    }
}

impl<'b, C> Decode<'b, C> for Summary {
    fn decode(d: &mut Decoder<'b>, _ctx: &mut C) -> Result<Self, minicbor::decode::Error> {
        let _ = d.array()?;
        let start = d.decode()?;
        let end = d.decode()?;
        let params = d.decode()?;
        d.skip()?;
        Ok(Summary { start, end, params })
    }
}

// A complete history of eras that have taken place.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct EraHistory {
    /// Number of slots for which the chain growth property guarantees at least k blocks.
    ///
    /// This is defined as 3 * k / f, where:
    ///
    /// - k is the network security parameter (mainnet = 2160);
    /// - f is the active slot coefficient (mainnet = 0.05);
    stability_window: Slot,

    /// Summary of each era boundaries.
    eras: Vec<Summary>,
}

impl<C> Encode<C> for EraHistory {
    fn encode<W: minicbor::encode::Write>(
        &self,
        e: &mut minicbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), minicbor::encode::Error<W::Error>> {
        e.begin_array()?;
        for s in &self.eras {
            s.encode(e, ctx)?;
        }
        e.end()?;
        Ok(())
    }
}

impl<'b> Decode<'b, Slot> for EraHistory {
    fn decode(d: &mut Decoder<'b>, ctx: &mut Slot) -> Result<Self, minicbor::decode::Error> {
        let mut eras = vec![];
        let eras_iter: minicbor::decode::ArrayIter<'_, '_, Summary> = d.array_iter()?;
        for era in eras_iter {
            eras.push(era?);
        }
        Ok(EraHistory {
            stability_window: *ctx,
            eras,
        })
    }
}

#[derive(Debug, PartialEq, Eq, thiserror::Error, serde::Serialize, serde::Deserialize)]
pub enum EraHistoryError {
    #[error("slot past time horizon")]
    PastTimeHorizon,
    #[error("invalid era history")]
    InvalidEraHistory,
    #[error("{0}")]
    SlotArithmetic(#[from] SlotArithmeticError),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EpochBounds {
    pub start: Slot,
    pub end: Option<Slot>,
}

// The last era in the provided EraHistory must end at the time horizon for accurate results. The
// horizon is the end of the epoch containing the end of the current era's safe zone relative to
// the current tip. Returns number of milliseconds elapsed since the system start time.
impl EraHistory {
    pub fn new(eras: &[Summary], stability_window: Slot) -> EraHistory {
        #[allow(clippy::panic)]
        if eras.is_empty() {
            panic!("EraHistory cannot be empty");
        }
        // TODO ensures only last era ends with Option
        EraHistory {
            stability_window,
            eras: eras.to_vec(),
        }
    }

    pub fn slot_to_relative_time(&self, slot: Slot, tip: Slot) -> Result<u64, EraHistoryError> {
        for era in &self.eras {
            if era.start.slot > slot {
                return Err(EraHistoryError::InvalidEraHistory);
            }

            if era.contains_slot(&slot, &tip, &self.stability_window) {
                return slot_to_relative_time(&slot, era);
            }
        }

        Err(EraHistoryError::PastTimeHorizon)
    }

    /// Unsafe version of `slot_to_relative_time` which doesn't check whether the slot is within a
    /// foreseeable horizon. Only use this when the slot is guaranteed to be in the past.
    pub fn slot_to_relative_time_unchecked_horizon(
        &self,
        slot: Slot,
    ) -> Result<u64, EraHistoryError> {
        for era in &self.eras {
            if era.start.slot > slot {
                return Err(EraHistoryError::InvalidEraHistory);
            }

            if era.contains_slot_unchecked_horizon(&slot) {
                return slot_to_relative_time(&slot, era);
            }
        }

        Err(EraHistoryError::InvalidEraHistory)
    }

    pub fn slot_to_epoch(&self, slot: Slot, tip: Slot) -> Result<Epoch, EraHistoryError> {
        for era in &self.eras {
            if era.start.slot > slot {
                return Err(EraHistoryError::InvalidEraHistory);
            }

            if era.contains_slot(&slot, &tip, &self.stability_window) {
                return slot_to_epoch(&slot, era);
            }
        }

        Err(EraHistoryError::PastTimeHorizon)
    }

    pub fn slot_to_epoch_unchecked_horizon(&self, slot: Slot) -> Result<Epoch, EraHistoryError> {
        for era in &self.eras {
            if era.start.slot > slot {
                return Err(EraHistoryError::InvalidEraHistory);
            }

            if era.contains_slot_unchecked_horizon(&slot) {
                return slot_to_epoch(&slot, era);
            }
        }

        Err(EraHistoryError::InvalidEraHistory)
    }

    pub fn next_epoch_first_slot(&self, epoch: Epoch, tip: &Slot) -> Result<Slot, EraHistoryError> {
        for era in &self.eras {
            if era.start.epoch > epoch {
                return Err(EraHistoryError::InvalidEraHistory);
            }

            if era.contains_epoch(&epoch, tip, &self.stability_window) {
                let start_of_next_epoch = match era.start.slot {
                    Slot(s) => (epoch.0 - era.start.epoch.0 + 1) * era.params.epoch_size_slots + s,
                };
                return Ok(Slot(start_of_next_epoch));
            }
        }
        Err(EraHistoryError::PastTimeHorizon)
    }

    /// Find the first epoch of the era from which this epoch belongs.
    pub fn era_first_epoch(&self, epoch: Epoch) -> Result<Epoch, EraHistoryError> {
        for era in &self.eras {
            // NOTE: This is okay. If there's no upper-bound to the era and the slot is after the
            // start of it, then necessarily the era's lower bound is what we're looking for.
            if era.contains_epoch_unchecked_horizon(&epoch) {
                return Ok(era.start.epoch);
            }
        }

        Err(EraHistoryError::InvalidEraHistory)
    }

    pub fn epoch_bounds(&self, epoch: Epoch) -> Result<EpochBounds, EraHistoryError> {
        for era in &self.eras {
            if era.start.epoch > epoch {
                return Err(EraHistoryError::InvalidEraHistory);
            }

            // NOTE: Unchecked horizon is okay here since in case there's no upper-bound, we'll
            // simply return a `None` epoch bounds as well.
            if era.contains_epoch_unchecked_horizon(&epoch) {
                let epochs_elapsed = epoch - era.start.epoch;
                let offset = era.start.slot;
                let slots_elapsed = epochs_elapsed * era.params.epoch_size_slots;
                let start = offset.offset_by(slots_elapsed);
                let end = offset.offset_by(era.params.epoch_size_slots + slots_elapsed);
                return Ok(EpochBounds {
                    start,
                    end: era.end.as_ref().map(|_| end),
                });
            }
        }

        Err(EraHistoryError::InvalidEraHistory)
    }

    /// Computes the relative slot in the epoch given an absolute slot.
    ///
    /// Returns the number of slots since the start of the epoch containing the given slot.
    ///
    /// # Errors
    ///
    /// Returns `EraHistoryError::PastTimeHorizon` if the slot is beyond the time horizon.
    /// Returns `EraHistoryError::InvalidEraHistory` if the era history is invalid.
    pub fn slot_in_epoch(&self, slot: Slot, tip: Slot) -> Result<Slot, EraHistoryError> {
        let epoch = self.slot_to_epoch(slot, tip)?;
        let bounds = self.epoch_bounds(epoch)?;
        let elapsed = slot.elapsed_from(bounds.start)?;
        Ok(Slot(elapsed))
    }
}

/// Compute the time in seconds between the start of the system and the given slot.
///
/// **pre-condition**: the given summary must be the era containing that slot.
fn slot_to_relative_time(slot: &Slot, era: &Summary) -> Result<u64, EraHistoryError> {
    let slots_elapsed = slot
        .elapsed_from(era.start.slot)
        .map_err(|_| EraHistoryError::InvalidEraHistory)?;
    let time_elapsed = era.params.slot_length * slots_elapsed;
    let relative_time = era.start.time_ms + time_elapsed;
    Ok(relative_time)
}

/// Compute the epoch corresponding to the given slot.
///
/// **pre-condition**: the given summary must be the era containing that slot.
fn slot_to_epoch(slot: &Slot, era: &Summary) -> Result<Epoch, EraHistoryError> {
    let slots_elapsed = slot
        .elapsed_from(era.start.slot)
        .map_err(|_| EraHistoryError::InvalidEraHistory)?;
    let epochs_elapsed = slots_elapsed / era.params.epoch_size_slots;
    let epoch_number = era.start.epoch + epochs_elapsed;
    Ok(epoch_number)
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::cmp::{max, min};
    use test_case::test_case;

    prop_compose! {
        fn arbitrary_bound()(time_ms in any::<u64>(), slot in any::<u64>(), epoch in any::<Epoch>()) -> Bound {
            Bound {
                time_ms, slot: Slot(slot), epoch
            }
        }
    }
    prop_compose! {
        fn arbitrary_bound_for_epoch(epoch: Epoch)(time_ms in any::<u64>(), slot in any::<u64>()) -> Bound {
            Bound {
                time_ms, slot: Slot(slot), epoch
            }
        }
    }
    prop_compose! {
        fn arbitrary_era_params()(epoch_size_slots in 1u64..65535u64, slot_length in 1u64..65535u64) -> EraParams {
            EraParams {
                epoch_size_slots,
                slot_length,
            }
        }
    }

    prop_compose! {
        fn arbitrary_summary()(
            b1 in any::<u16>(),
            b2 in any::<u16>(),
            params in arbitrary_era_params(),
        )(
            first_epoch in Just(min(b1, b2) as u64),
            last_epoch in Just(max(b1, b2) as u64),
            params in Just(params),
            start in arbitrary_bound_for_epoch(Epoch::from(max(b1, b2) as u64)),
        ) -> Summary {
                let epochs_elapsed = last_epoch - first_epoch;
                let slots_elapsed = epochs_elapsed * params.epoch_size_slots;
                let time_elapsed = slots_elapsed * params.slot_length;
                let end = Some(Bound {
                    time_ms: start.time_ms + time_elapsed,
                    slot: start.slot.offset_by(slots_elapsed),
                    epoch: Epoch::from(last_epoch),
                });
                Summary { start, end, params }
            }
    }

    prop_compose! {
        // Generate an arbitrary list of ordered epochs where we might have a new era
        fn arbitrary_boundaries()(
            first_epoch in any::<u16>(),
            era_lengths in prop::collection::vec(1u64..1000, 1usize..32usize),
        ) -> Vec<u64> {
            let mut boundaries = vec![first_epoch as u64];
            for era_length in era_lengths {
                boundaries.push(boundaries.last().unwrap() + era_length);
            }
            boundaries
        }
    }

    prop_compose! {
        fn arbitrary_era_history()(
            boundaries in arbitrary_boundaries()
        )(
            stability_window in any::<u64>(),
            era_params in prop::collection::vec(arbitrary_era_params(), boundaries.len()),
            boundaries in Just(boundaries),
        ) -> EraHistory {
            let genesis = Bound::genesis();

            let mut prev_bound = genesis;

            // For each boundary, compute the time and slot for that epoch based on the era params and
            // construct a summary from the boundary pair
            let mut summaries = vec![];
            for (boundary, prev_era_params) in boundaries.iter().zip(era_params.iter()) {
                let epochs_elapsed = boundary - prev_bound.epoch.0;
                let slots_elapsed = epochs_elapsed * prev_era_params.epoch_size_slots;
                let time_elapsed = slots_elapsed * prev_era_params.slot_length;
                let new_bound = Bound {
                    time_ms: prev_bound.time_ms + time_elapsed,
                    slot: prev_bound.slot.offset_by(slots_elapsed),
                    epoch: Epoch(*boundary),
                };

                summaries.push(Summary {
                    start: prev_bound,
                    end: if *boundary as usize == boundaries.len() {
                        None
                    } else {
                        Some(new_bound.clone())
                    },
                    params: prev_era_params.clone(),
                });

                prev_bound = new_bound;
            }

            EraHistory::new(&summaries, Slot::from(stability_window))
        }
    }

    fn default_params() -> EraParams {
        EraParams::new(86400, 1000).unwrap()
    }

    fn one_era() -> EraHistory {
        EraHistory {
            stability_window: Slot(25920),
            eras: vec![Summary {
                start: Bound {
                    time_ms: 0,
                    slot: Slot(0),
                    epoch: Epoch(0),
                },
                end: None,
                params: default_params(),
            }],
        }
    }

    fn two_eras() -> EraHistory {
        EraHistory {
            stability_window: Slot(25920),
            eras: vec![
                Summary {
                    start: Bound {
                        time_ms: 0,
                        slot: Slot(0),
                        epoch: Epoch(0),
                    },
                    end: Some(Bound {
                        time_ms: 86400000,
                        slot: Slot(86400),
                        epoch: Epoch(1),
                    }),
                    params: default_params(),
                },
                Summary {
                    start: Bound {
                        time_ms: 86400000,
                        slot: Slot(86400),
                        epoch: Epoch(1),
                    },
                    end: None,
                    params: default_params(),
                },
            ],
        }
    }

    #[test]
    fn slot_to_relative_time_within_horizon() {
        let eras = two_eras();
        assert_eq!(
            eras.slot_to_relative_time(Slot(172800), Slot(172800)),
            Ok(172800000)
        );
    }

    #[test]
    fn slot_to_time_fails_after_time_horizon() {
        let eras = two_eras();
        assert_eq!(
            eras.slot_to_relative_time(Slot(172800), Slot(100000)),
            Ok(172800000),
            "point is right at the end of the epoch, tip is somewhere"
        );
        assert_eq!(
            eras.slot_to_relative_time(Slot(172801), Slot(86400)),
            Err(EraHistoryError::PastTimeHorizon),
            "point in the next epoch, but tip is way before the stability window (first slot)"
        );
        assert_eq!(
            eras.slot_to_relative_time(Slot(172801), Slot(100000)),
            Err(EraHistoryError::PastTimeHorizon),
            "point in the next epoch, but tip is way before the stability window (somewhere)"
        );
        assert_eq!(
            eras.slot_to_relative_time(Slot(172801), Slot(146880)),
            Ok(172801000),
            "point in the next epoch, and tip right at the stability window limit"
        );
        assert_eq!(
            eras.slot_to_relative_time(Slot(172801), Slot(146879)),
            Err(EraHistoryError::PastTimeHorizon),
            "point in the next epoch, but tip right *before* the stability window limit"
        );
    }

    #[test]
    fn epoch_bounds_epoch_0() {
        let bounds = two_eras().epoch_bounds(Epoch(0)).unwrap();
        assert_eq!(bounds.start, Slot(0));
        assert_eq!(bounds.end, Some(Slot(86400)));
    }

    #[test]
    fn epoch_bounds_epoch_1() {
        let bounds = two_eras().epoch_bounds(Epoch(1)).unwrap();
        assert_eq!(bounds.start, Slot(86400));
        assert_eq!(bounds.end, None);
    }

    #[test_case(0,          42 => Ok(0);
        "first slot in first epoch, tip irrelevant"
    )]
    #[test_case(48272,      42 => Ok(0);
        "slot anywhere in first epoch, tip irrelevant"
    )]
    #[test_case(86400,      42 => Ok(1);
        "first slot in second epoch, tip irrelevant"
    )]
    #[test_case(105437,     42 => Ok(1);
        "slot anywhere in second epoch, tip irrelevant"
    )]
    #[test_case(172801, 146879 => Err(EraHistoryError::PastTimeHorizon);
        "slot beyond first epoch (at the frontier), tip before stable area"
    )]
    #[test_case(200000, 146879 => Err(EraHistoryError::PastTimeHorizon);
        "slot beyond first epoch (anywhere), tip before stable area"
    )]
    #[test_case(200000, 146880 => Ok(2);
        "slot within third epoch, tip in stable area (lower frontier)"
    )]
    #[test_case(200000, 153129 => Ok(2);
        "slot within third epoch, tip in stable area (anywhere)"
    )]
    #[test_case(200000, 172800 => Ok(2);
        "slot within third epoch, tip in stable area (upper frontier)"
    )]
    #[test_case(260000, 146880 => Err(EraHistoryError::PastTimeHorizon);
        "slot far far away, tip in stable area (lower frontier)"
    )]
    #[test_case(260000, 153129 => Err(EraHistoryError::PastTimeHorizon);
        "slot far far away, tip in stable area (anywhere)"
    )]
    #[test_case(260000, 172800 => Err(EraHistoryError::PastTimeHorizon);
        "slot far far away, tip in stable area (upper frontier)"
    )]
    fn slot_to_epoch(slot: u64, tip: u64) -> Result<u64, EraHistoryError> {
        two_eras()
            .slot_to_epoch(Slot(slot), Slot(tip))
            .map(|Epoch(e)| e)
    }

    #[test_case(0 => Ok(0); "first slot in first epoch")]
    #[test_case(48272 => Ok(0); "slot anywhere in first epoch")]
    #[test_case(86400 => Ok(1); "first slot in second epoch")]
    #[test_case(105437 => Ok(1); "slot anywhere in second epoch")]
    #[test_case(172801 => Ok(2); "slot beyond first epoch (at the frontier)")]
    #[test_case(200000 => Ok(2); "slot within third epoch")]
    #[test_case(260000 => Ok(3); "slot far far away")]
    fn slot_to_epoch_unchecked_horizon(slot: u64) -> Result<u64, EraHistoryError> {
        two_eras()
            .slot_to_epoch_unchecked_horizon(Slot(slot))
            .map(|Epoch(e)| e)
    }

    #[test_case(0, 42 => Ok(86400); "fully known forecast (1), tip irrelevant")]
    #[test_case(1, 42 => Ok(172800); "fully known forecast (2), tip irrelevant")]
    #[test_case(2, 42 => Err(EraHistoryError::PastTimeHorizon);
        "far away forecast, tip before stable window (well before)"
    )]
    #[test_case(2, 146879 => Err(EraHistoryError::PastTimeHorizon);
        "far away forecast, tip before stable window (lower frontier)"
    )]
    #[test_case(2, 146880 => Ok(259200);
        "far away forecast, tip within stable window (lower frontier)"
    )]
    #[test_case(2, 201621 => Ok(259200);
        "far away forecast, tip within stable window (anywhere)"
    )]
    #[test_case(2, 259199 => Ok(259200);
        "far away forecast, tip within stable window (upper frontier)"
    )]
    fn next_epoch_first_slot(epoch: u64, tip: u64) -> Result<u64, EraHistoryError> {
        two_eras()
            .next_epoch_first_slot(Epoch(epoch), &Slot(tip))
            .map(|Slot(s)| s)
    }

    #[test]
    fn slot_in_epoch_invalid_era_history() {
        // Create an invalid era history where the second era starts at a slot
        // that is earlier than the first era's start slot
        let invalid_eras = EraHistory {
            stability_window: Slot(129600),
            eras: vec![
                Summary {
                    start: Bound {
                        time_ms: 100000,
                        slot: Slot(100),
                        epoch: Epoch(1),
                    },
                    end: Some(Bound {
                        time_ms: 186400000,
                        slot: Slot(86500),
                        epoch: Epoch(2),
                    }),
                    params: default_params(),
                },
                Summary {
                    start: Bound {
                        time_ms: 186400000,
                        slot: Slot(50), // This is invalid - earlier than first era's start
                        epoch: Epoch(2),
                    },
                    end: Some(Bound {
                        time_ms: 272800000,
                        slot: Slot(86450),
                        epoch: Epoch(3),
                    }),
                    params: default_params(),
                },
            ],
        };

        let relative_slot = invalid_eras.slot_in_epoch(Slot(60), Slot(42));
        assert_eq!(relative_slot, Err(EraHistoryError::InvalidEraHistory));
    }

    #[test]
    fn slot_in_epoch_underflows_given_era_history_with_gaps() {
        // Create a custom era history with a gap between epochs
        let invalid_eras = EraHistory {
            stability_window: Slot(129600),
            eras: vec![
                Summary {
                    start: Bound {
                        time_ms: 0,
                        slot: Slot(0),
                        epoch: Epoch(0),
                    },
                    end: Some(Bound {
                        time_ms: 86400000,
                        slot: Slot(86400),
                        epoch: Epoch(1),
                    }),
                    params: default_params(),
                },
                Summary {
                    start: Bound {
                        time_ms: 86400000,
                        slot: Slot(186400), // Gap of 100000 slots
                        epoch: Epoch(1),
                    },
                    end: Some(Bound {
                        time_ms: 172800000,
                        slot: Slot(272800),
                        epoch: Epoch(2),
                    }),
                    params: default_params(),
                },
            ],
        };

        // A slot in epoch 1 but before the start of epoch 1's slots
        let problematic_slot = Slot(100000);

        let result = invalid_eras.slot_in_epoch(problematic_slot, Slot(42));
        assert_eq!(result, Err(EraHistoryError::InvalidEraHistory));
    }

    #[test]
    fn encode_era_history() {
        let eras = one_era();
        let buffer = minicbor::to_vec(&eras).unwrap();
        assert_eq!(
            hex::encode(buffer),
            "9f9f9f000000fff69f1a000151801903e8ffffff"
        );
    }

    proptest! {
        #[test]
        fn roundtrip_era_history(era_history in arbitrary_era_history()) {
            let buffer = minicbor::to_vec(&era_history).unwrap();
            let decoded = minicbor::decode_with(&buffer, &mut era_history.stability_window.clone()).unwrap();
            assert_eq!(era_history, decoded);
        }
    }
}
