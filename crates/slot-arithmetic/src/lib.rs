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

use std::{
    fmt::Display,
    iter::Step,
    ops::{Add, Sub},
    str::FromStr,
};

use minicbor::{Decode, Decoder, Encode};

use serde::{Deserialize, Serialize};

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
    pub end: Bound,
    pub params: EraParams,
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
    pub eras: Vec<Summary>,
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

impl<'b, C> Decode<'b, C> for EraHistory {
    fn decode(d: &mut Decoder<'b>, _ctx: &mut C) -> Result<Self, minicbor::decode::Error> {
        let mut eras = vec![];
        let eras_iter: minicbor::decode::ArrayIter<'_, '_, Summary> = d.array_iter()?;
        for era in eras_iter {
            eras.push(era?);
        }
        Ok(EraHistory { eras })
    }
}

#[derive(Debug, PartialEq, Eq, thiserror::Error, serde::Serialize, serde::Deserialize)]
pub enum TimeHorizonError {
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
    pub end: Slot,
}

// The last era in the provided EraHistory must end at the time horizon for accurate results. The
// horizon is the end of the epoch containing the end of the current era's safe zone relative to
// the current tip. Returns number of milliseconds elapsed since the system start time.
impl EraHistory {
    pub fn slot_to_relative_time(&self, slot: Slot) -> Result<u64, TimeHorizonError> {
        for era in &self.eras {
            if era.start.slot > slot {
                return Err(TimeHorizonError::InvalidEraHistory);
            }
            if era.end.slot >= slot {
                let slots_elapsed = slot
                    .elapsed_from(era.start.slot)
                    .map_err(|_| TimeHorizonError::InvalidEraHistory)?;
                let time_elapsed = era.params.slot_length * slots_elapsed;
                let relative_time = era.start.time_ms + time_elapsed;
                return Ok(relative_time);
            }
        }
        Err(TimeHorizonError::PastTimeHorizon)
    }

    pub fn slot_to_absolute_time(
        &self,
        slot: Slot,
        system_start: u64,
    ) -> Result<u64, TimeHorizonError> {
        self.slot_to_relative_time(slot).map(|t| system_start + t)
    }

    pub fn relative_time_to_slot(&self, time: u64) -> Result<Slot, TimeHorizonError> {
        for era in &self.eras {
            if era.start.time_ms > time {
                return Err(TimeHorizonError::InvalidEraHistory);
            }
            if era.end.time_ms >= time {
                let time_elapsed = time - era.start.time_ms;
                let slots_elapsed = time_elapsed / era.params.slot_length;
                let slot = era.start.slot.offset_by(slots_elapsed);
                return Ok(slot);
            }
        }
        Err(TimeHorizonError::PastTimeHorizon)
    }

    pub fn slot_to_epoch(&self, slot: Slot) -> Result<Epoch, TimeHorizonError> {
        for era in &self.eras {
            if era.start.slot > slot {
                return Err(TimeHorizonError::InvalidEraHistory);
            }
            if era.end.slot >= slot {
                let slots_elapsed = slot
                    .elapsed_from(era.start.slot)
                    .map_err(|_| TimeHorizonError::InvalidEraHistory)?;
                let epochs_elapsed = slots_elapsed / era.params.epoch_size_slots;
                let epoch_number = era.start.epoch + epochs_elapsed;
                return Ok(epoch_number);
            }
        }
        Err(TimeHorizonError::PastTimeHorizon)
    }

    pub fn next_epoch_first_slot(&self, epoch: Epoch) -> Result<Slot, TimeHorizonError> {
        for era in &self.eras {
            if era.start.epoch > epoch {
                return Err(TimeHorizonError::InvalidEraHistory);
            }
            if era.end.epoch > epoch {
                let start_of_next_epoch = match era.start.slot {
                    Slot(s) => (epoch.0 - era.start.epoch.0 + 1) * era.params.epoch_size_slots + s,
                };
                return Ok(Slot(start_of_next_epoch));
            }
        }
        Err(TimeHorizonError::PastTimeHorizon)
    }

    /// Find the first epoch of the era from which this epoch belongs.
    pub fn era_first_epoch(&self, epoch: Epoch) -> Result<Epoch, TimeHorizonError> {
        for era in &self.eras {
            if epoch < era.end.epoch {
                return Ok(era.start.epoch);
            }
        }
        Err(TimeHorizonError::PastTimeHorizon)
    }

    pub fn epoch_bounds(&self, epoch: Epoch) -> Result<EpochBounds, TimeHorizonError> {
        for era in &self.eras {
            if era.start.epoch > epoch {
                return Err(TimeHorizonError::InvalidEraHistory);
            }
            // We can't answer queries about the upper bound epoch of the era because the bound is
            // exclusive.
            if era.end.epoch > epoch {
                let epochs_elapsed = epoch - era.start.epoch;
                let offset = era.start.slot;
                let slots_elapsed = epochs_elapsed * era.params.epoch_size_slots;
                let start = offset.offset_by(slots_elapsed);
                let end = offset.offset_by(era.params.epoch_size_slots + slots_elapsed);
                return Ok(EpochBounds { start, end });
            }
        }
        Err(TimeHorizonError::PastTimeHorizon)
    }

    /// Computes the relative slot in the epoch given an absolute slot.
    ///
    /// Returns the number of slots since the start of the epoch containing the given slot.
    ///
    /// # Errors
    ///
    /// Returns `TimeHorizonError::PastTimeHorizon` if the slot is beyond the time horizon.
    /// Returns `TimeHorizonError::InvalidEraHistory` if the era history is invalid.
    pub fn slot_in_epoch(&self, slot: Slot) -> Result<Slot, TimeHorizonError> {
        let epoch = self.slot_to_epoch(slot)?;
        let bounds = self.epoch_bounds(epoch)?;
        let elapsed = slot.elapsed_from(bounds.start)?;
        Ok(Slot(elapsed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::cmp::{max, min};

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
                let end = Bound {
                    time_ms: start.time_ms + time_elapsed,
                    slot: start.slot.offset_by(slots_elapsed),
                    epoch: Epoch::from(last_epoch),
                };
                Summary { start, end, params }
            }
    }

    prop_compose! {
        // Generate an arbitrary list of ordered epochs where we might have a new era
        fn arbitrary_boundaries()(
            first_epoch in any::<u16>(),
            era_lengths in prop::collection::vec(1u64.., 1usize..32usize),
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
                    end: new_bound.clone(),
                    params: prev_era_params.clone(),
                });

                prev_bound = new_bound;
            }

            EraHistory { eras: summaries }
        }
    }

    fn default_params() -> EraParams {
        EraParams::new(86400, 1000).unwrap()
    }

    fn one_era() -> EraHistory {
        EraHistory {
            eras: vec![Summary {
                start: Bound {
                    time_ms: 0,
                    slot: Slot(0),
                    epoch: Epoch(0),
                },
                end: Bound {
                    time_ms: 864000000,
                    slot: Slot(864000),
                    epoch: Epoch(10),
                },
                params: default_params(),
            }],
        }
    }

    fn two_eras() -> EraHistory {
        EraHistory {
            eras: vec![
                Summary {
                    start: Bound {
                        time_ms: 0,
                        slot: Slot(0),
                        epoch: Epoch(0),
                    },
                    end: Bound {
                        time_ms: 86400000,
                        slot: Slot(86400),
                        epoch: Epoch(1),
                    },
                    params: default_params(),
                },
                Summary {
                    start: Bound {
                        time_ms: 86400000,
                        slot: Slot(86400),
                        epoch: Epoch(1),
                    },
                    end: Bound {
                        time_ms: 172800000,
                        slot: Slot(172800),
                        epoch: Epoch(2),
                    },
                    params: default_params(),
                },
            ],
        }
    }

    #[test]
    fn slot_to_time_example_1() {
        let eras = two_eras();
        let t0 = eras.slot_to_relative_time(Slot(172800));
        assert_eq!(t0, Ok(172800000));
    }

    #[test]
    fn slot_to_time_fails_after_time_horizon() {
        let eras = two_eras();
        let t0 = eras.slot_to_relative_time(Slot(172801));
        assert_eq!(t0, Err(TimeHorizonError::PastTimeHorizon));
    }

    #[test]
    fn epoch_bounds_example_1() {
        let eras = one_era();
        assert_eq!(eras.epoch_bounds(Epoch(1)).unwrap().start, Slot(86400));
    }

    #[test]
    fn epoch_bounds_example_2() {
        let eras = one_era();
        assert_eq!(eras.epoch_bounds(Epoch(1)).unwrap().end, Slot(172800));
    }

    #[test]
    fn epoch_bounds_fails_after_time_horizon() {
        let eras = one_era();
        assert_eq!(
            eras.epoch_bounds(Epoch(10)),
            Err(TimeHorizonError::PastTimeHorizon)
        );
    }

    #[test]
    fn slot_to_epoch_example_1() {
        let eras = one_era();
        let e = eras.slot_to_epoch(Slot(0));
        assert_eq!(e, Ok(Epoch(0)));
    }

    #[test]
    fn slot_to_epoch_example_2() {
        let eras = one_era();
        let e = eras.slot_to_epoch(Slot(86399));
        assert_eq!(e, Ok(Epoch(0)));
    }

    #[test]
    fn slot_to_epoch_example_3() {
        let eras = one_era();
        let e = eras.slot_to_epoch(Slot(864000));
        assert_eq!(e, Ok(Epoch(10)));
    }

    #[test]
    fn slot_to_epoch_fails_after_time_horizon() {
        let eras = one_era();
        let e = eras.slot_to_epoch(Slot(864001));
        assert_eq!(e, Err(TimeHorizonError::PastTimeHorizon));
    }

    #[test]
    fn next_epoch_first_slot() {
        let eras = one_era();
        assert_eq!(eras.next_epoch_first_slot(Epoch(0)), Ok(Slot(86400)));
    }

    #[test]
    fn next_epoch_first_slot_at_eras_boundary() {
        let eras = two_eras();
        assert_eq!(eras.next_epoch_first_slot(Epoch(1)), Ok(Slot(172800)));
    }

    #[test]
    fn next_epoch_first_slot_beyond_end_of_history() {
        let eras = two_eras();
        assert_eq!(
            eras.next_epoch_first_slot(Epoch(2)),
            Err(TimeHorizonError::PastTimeHorizon)
        );
    }

    #[test]
    fn slot_in_epoch_example_1() {
        let eras = one_era();
        let relative_slot = eras.slot_in_epoch(Slot(0));
        assert_eq!(relative_slot, Ok(Slot(0)));
    }

    #[test]
    fn slot_in_epoch_example_2() {
        let eras = one_era();
        let relative_slot = eras.slot_in_epoch(Slot(86400));
        assert_eq!(relative_slot, Ok(Slot(0)));
    }

    #[test]
    fn slot_in_epoch_example_3() {
        let eras = one_era();
        let relative_slot = eras.slot_in_epoch(Slot(86401));
        assert_eq!(relative_slot, Ok(Slot(1)));
    }

    #[test]
    fn slot_in_epoch_example_4() {
        let eras = one_era();
        let relative_slot = eras.slot_in_epoch(Slot(172799));
        assert_eq!(relative_slot, Ok(Slot(86399)));
    }

    #[test]
    fn slot_in_epoch_past_time_horizon() {
        let eras = one_era();
        let relative_slot = eras.slot_in_epoch(Slot(864001));
        assert_eq!(relative_slot, Err(TimeHorizonError::PastTimeHorizon));
    }

    #[test]
    fn slot_in_epoch_invalid_era_history() {
        // Create an invalid era history where the second era starts at a slot
        // that is earlier than the first era's start slot
        let invalid_eras = EraHistory {
            eras: vec![
                Summary {
                    start: Bound {
                        time_ms: 100000,
                        slot: Slot(100),
                        epoch: Epoch(1),
                    },
                    end: Bound {
                        time_ms: 186400000,
                        slot: Slot(86500),
                        epoch: Epoch(2),
                    },
                    params: default_params(),
                },
                Summary {
                    start: Bound {
                        time_ms: 186400000,
                        slot: Slot(50), // This is invalid - earlier than first era's start
                        epoch: Epoch(2),
                    },
                    end: Bound {
                        time_ms: 272800000,
                        slot: Slot(86450),
                        epoch: Epoch(3),
                    },
                    params: default_params(),
                },
            ],
        };

        let relative_slot = invalid_eras.slot_in_epoch(Slot(60));
        assert_eq!(relative_slot, Err(TimeHorizonError::InvalidEraHistory));
    }

    #[test]
    fn slot_in_epoch_underflows_given_era_history_with_gaps() {
        // Create a custom era history with a gap between epochs
        let invalid_eras = EraHistory {
            eras: vec![
                Summary {
                    start: Bound {
                        time_ms: 0,
                        slot: Slot(0),
                        epoch: Epoch(0),
                    },
                    end: Bound {
                        time_ms: 86400000,
                        slot: Slot(86400),
                        epoch: Epoch(1),
                    },
                    params: default_params(),
                },
                Summary {
                    start: Bound {
                        time_ms: 86400000,
                        slot: Slot(186400), // Gap of 100000 slots
                        epoch: Epoch(1),
                    },
                    end: Bound {
                        time_ms: 172800000,
                        slot: Slot(272800),
                        epoch: Epoch(2),
                    },
                    params: default_params(),
                },
            ],
        };

        // A slot in epoch 1 but before the start of epoch 1's slots
        let problematic_slot = Slot(100000);

        let result = invalid_eras.slot_in_epoch(problematic_slot);
        assert_eq!(result, Err(TimeHorizonError::InvalidEraHistory));
    }

    #[test]
    fn encode_era_history() {
        let eras = one_era();
        let buffer = minicbor::to_vec(&eras).unwrap();
        assert_eq!(
            hex::encode(buffer),
            "9f9f9f000000ff9f1a337f98001a000d2f000aff9f1a000151801903e8ffffff"
        );
    }

    proptest! {
        fn roundtrip_era_history(era_history in arbitrary_era_history()) {
            let buffer = minicbor::to_vec(&era_history).unwrap();
            let decoded = minicbor::decode(&buffer).unwrap();
            assert_eq!(era_history, decoded);
        }
    }
}

#[cfg(feature = "test-utils")]
pub mod testing;
