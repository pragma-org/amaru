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

use minicbor::{Decode, Decoder, Encode};
use serde::{Deserialize, Serialize};

type Slot = u64;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bound {
    pub time_ms: u64, // Milliseconds
    pub slot: Slot,
    pub epoch: u64,
}

#[allow(dead_code)]
impl Bound {
    fn genesis() -> Bound {
        Bound {
            time_ms: 0,
            slot: 0,
            epoch: 0,
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
    fn decode(d: &mut Decoder<'b>, _ctx: &mut C) -> Result<Self, minicbor::decode::Error> {
        let _ = d.array()?;
        let time_ms = d.u64()?;
        let slot = d.u64()?;
        let epoch = d.u64()?;
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum TimeHorizonError {
    #[error("slot past time horizon")]
    PastTimeHorizon,
    #[error("invalid era history")]
    InvalidEraHistory,
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
    pub fn slot_to_relative_time(&self, slot: u64) -> Result<u64, TimeHorizonError> {
        for era in &self.eras {
            if era.start.slot > slot {
                return Err(TimeHorizonError::InvalidEraHistory);
            }
            if era.end.slot >= slot {
                let slots_elapsed = slot - era.start.slot;
                let time_elapsed = era.params.slot_length * slots_elapsed;
                let relative_time = era.start.time_ms + time_elapsed;
                return Ok(relative_time);
            }
        }
        Err(TimeHorizonError::PastTimeHorizon)
    }

    pub fn slot_to_absolute_time(
        &self,
        slot: u64,
        system_start: u64,
    ) -> Result<u64, TimeHorizonError> {
        self.slot_to_relative_time(slot).map(|t| system_start + t)
    }

    pub fn relative_time_to_slot(&self, time: u64) -> Result<u64, TimeHorizonError> {
        for era in &self.eras {
            if era.start.time_ms > time {
                return Err(TimeHorizonError::InvalidEraHistory);
            }
            if era.end.time_ms >= time {
                let time_elapsed = time - era.start.time_ms;
                let slots_elapsed = time_elapsed / era.params.slot_length;
                let slot = era.start.slot + slots_elapsed;
                return Ok(slot);
            }
        }
        Err(TimeHorizonError::PastTimeHorizon)
    }

    pub fn slot_to_epoch(&self, slot: u64) -> Result<u64, TimeHorizonError> {
        for era in &self.eras {
            if era.start.slot > slot {
                return Err(TimeHorizonError::InvalidEraHistory);
            }
            if era.end.slot >= slot {
                let slots_elapsed = slot - era.start.slot;
                let epochs_elapsed = slots_elapsed / era.params.epoch_size_slots;
                let epoch_number = era.start.epoch + epochs_elapsed;
                return Ok(epoch_number);
            }
        }
        Err(TimeHorizonError::PastTimeHorizon)
    }

    pub fn next_epoch_first_slot(&self, epoch: u64) -> Result<u64, TimeHorizonError> {
        for era in &self.eras {
            if era.start.epoch > epoch {
                return Err(TimeHorizonError::InvalidEraHistory);
            }
            if era.end.epoch > epoch {
                let start_of_next_epoch =
                    (epoch - era.start.epoch + 1) * era.params.epoch_size_slots + era.start.slot;
                return Ok(start_of_next_epoch);
            }
        }
        Err(TimeHorizonError::PastTimeHorizon)
    }

    pub fn epoch_bounds(&self, epoch: u64) -> Result<EpochBounds, TimeHorizonError> {
        for era in &self.eras {
            if era.start.epoch > epoch {
                return Err(TimeHorizonError::InvalidEraHistory);
            }
            // We can't answer queries about the upper bound epoch of the era because the bound is
            // exclusive.
            if era.end.epoch > epoch {
                let epochs_elapsed = epoch - era.start.epoch;
                let offset = era.start.slot;
                let start = offset + era.params.epoch_size_slots * epochs_elapsed;
                let end = offset + era.params.epoch_size_slots * (epochs_elapsed + 1);
                return Ok(EpochBounds { start, end });
            }
        }
        Err(TimeHorizonError::PastTimeHorizon)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::cmp::{max, min};

    prop_compose! {
        fn arbitrary_bound()(time_ms in any::<u64>(), slot in any::<u64>(), epoch in any::<u64>()) -> Bound {
            Bound {
                time_ms, slot, epoch
            }
        }
    }
    prop_compose! {
        fn arbitrary_bound_for_epoch(epoch: u64)(time_ms in any::<u64>(), slot in any::<u64>()) -> Bound {
            Bound {
                time_ms, slot, epoch
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
            start in arbitrary_bound_for_epoch(max(b1, b2) as u64),
        ) -> Summary {
                let epochs_elapsed = last_epoch - first_epoch;
                let slots_elapsed = epochs_elapsed * params.epoch_size_slots;
                let time_elapsed = slots_elapsed * params.slot_length;
                let end = Bound {
                    time_ms: start.time_ms + time_elapsed,
                    slot: start.slot + slots_elapsed,
                    epoch: last_epoch,
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
                let epochs_elapsed = boundary - prev_bound.epoch;
                let slots_elapsed = epochs_elapsed * prev_era_params.epoch_size_slots;
                let time_elapsed = slots_elapsed * prev_era_params.slot_length;
                let new_bound = Bound {
                    time_ms: prev_bound.time_ms + time_elapsed,
                    slot: prev_bound.slot + slots_elapsed,
                    epoch: *boundary,
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
                    slot: 0,
                    epoch: 0,
                },
                end: Bound {
                    time_ms: 864000000,
                    slot: 864000,
                    epoch: 10,
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
                        slot: 0,
                        epoch: 0,
                    },
                    end: Bound {
                        time_ms: 86400000,
                        slot: 86400,
                        epoch: 1,
                    },
                    params: default_params(),
                },
                Summary {
                    start: Bound {
                        time_ms: 86400000,
                        slot: 86400,
                        epoch: 1,
                    },
                    end: Bound {
                        time_ms: 172800000,
                        slot: 172800,
                        epoch: 2,
                    },
                    params: default_params(),
                },
            ],
        }
    }

    #[test]
    fn slot_to_time_example_1() {
        let eras = two_eras();
        let t0 = eras.slot_to_relative_time(172800);
        assert_eq!(t0, Ok(172800000));
    }

    #[test]
    fn slot_to_time_fails_after_time_horizon() {
        let eras = two_eras();
        let t0 = eras.slot_to_relative_time(172801);
        assert_eq!(t0, Err(TimeHorizonError::PastTimeHorizon));
    }

    #[test]
    fn epoch_bounds_example_1() {
        let eras = one_era();
        assert_eq!(eras.epoch_bounds(1).unwrap().start, 86400);
    }

    #[test]
    fn epoch_bounds_example_2() {
        let eras = one_era();
        assert_eq!(eras.epoch_bounds(1).unwrap().end, 172800);
    }

    #[test]
    fn epoch_bounds_fails_after_time_horizon() {
        let eras = one_era();
        assert_eq!(
            eras.epoch_bounds(10),
            Err(TimeHorizonError::PastTimeHorizon)
        );
    }

    #[test]
    fn slot_to_epoch_example_1() {
        let eras = one_era();
        let e = eras.slot_to_epoch(0);
        assert_eq!(e, Ok(0));
    }

    #[test]
    fn slot_to_epoch_example_2() {
        let eras = one_era();
        let e = eras.slot_to_epoch(86399);
        assert_eq!(e, Ok(0));
    }

    #[test]
    fn slot_to_epoch_example_3() {
        let eras = one_era();
        let e = eras.slot_to_epoch(864000);
        assert_eq!(e, Ok(10));
    }

    #[test]
    fn slot_to_epoch_fails_after_time_horizon() {
        let eras = one_era();
        let e = eras.slot_to_epoch(864001);
        assert_eq!(e, Err(TimeHorizonError::PastTimeHorizon));
    }

    #[test]
    fn next_epoch_first_slot() {
        let eras = one_era();
        assert_eq!(eras.next_epoch_first_slot(0), Ok(86400));
    }

    #[test]
    fn next_epoch_first_slot_at_eras_boundary() {
        let eras = two_eras();
        assert_eq!(eras.next_epoch_first_slot(1), Ok(172800));
    }

    #[test]
    fn next_epoch_first_slot_beyond_end_of_history() {
        let eras = two_eras();
        assert_eq!(
            eras.next_epoch_first_slot(2),
            Err(TimeHorizonError::PastTimeHorizon)
        );
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
