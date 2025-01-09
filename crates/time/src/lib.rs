use minicbor::{Encode, Decode, Decoder};
use quickcheck::{Arbitrary, Gen};
use std::cmp::{max, min};

type Slot = u64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bound {
    pub time_ms: u64, // Milliseconds
    pub slot: Slot,
    pub epoch: u64,
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
        let _ = d.skip()?;
        Ok(Bound {
            time_ms,
            slot,
            epoch,
        })
    }
}

impl Arbitrary for Bound {
    fn arbitrary(g: &mut Gen) -> Self {
        Bound {
            time_ms: u64::arbitrary(g),
            slot: u64::arbitrary(g),
            epoch: u64::arbitrary(g),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EraParams {
    epoch_size_slots: u64,
    slot_length: u64, // Milliseconds
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
        let _ = d.skip()?;
        Ok(EraParams {
            epoch_size_slots,
            slot_length,
        })
    }
}

impl Arbitrary for EraParams {
    fn arbitrary(g: &mut Gen) -> Self {
        EraParams {
            // An epoch can't be zero slots
            epoch_size_slots: max(1, u16::arbitrary(g) as u64),
            // A slot can't be zero milliseconds
            slot_length: max(1, u16::arbitrary(g) as u64),
        }
    }
}

// The start is inclusive and the end is exclusive. In a valid EraHistory, the
// end of each era will equal the start of the next one.
#[derive(Clone, Debug, PartialEq, Eq)]
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
        let _ = d.skip()?;
        Ok(Summary {
            start,
            end,
            params,
        })
    }
}

impl Arbitrary for Summary {
    fn arbitrary(g: &mut Gen) -> Summary {
        let b1 = u16::arbitrary(g) as u64;
        let b2 = u16::arbitrary(g) as u64;
        let params = EraParams::arbitrary(g);
        let start = Bound {
            time_ms: u16::arbitrary(g) as u64,
            slot: u16::arbitrary(g) as u64,
            epoch: min(b1, b2),
        };
        let epochs_elapsed = max(b1, b2) - min(b1, b2);
        let slots_elapsed = epochs_elapsed * params.epoch_size_slots;
        let time_elapsed = slots_elapsed * params.slot_length;
        let end = Bound {
            time_ms: start.time_ms + time_elapsed,
            slot: start.slot + slots_elapsed,
            epoch: max(b1, b2),
        };
        Summary {
            start,
            end,
            params,
        }
    }
}

// A complete history of eras that have taken place.
#[derive(Clone, Debug, PartialEq, Eq)]
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
        let eras_iter: minicbor::decode::ArrayIter<Summary> = d.array_iter()?;
        for era in eras_iter {
            eras.push(era?);
        }
        Ok(EraHistory {
            eras,
        })
    }
}

impl Arbitrary for EraHistory {
    fn arbitrary(g: &mut Gen) -> EraHistory {
        // Generate a number of boundaries corresponding to the generator size
        let mut boundaries = vec![];
        boundaries.push(u16::arbitrary(g) as u64);
        for i in 0..g.size() {
            // An era can't be zero-length
            boundaries.push(boundaries[i] + max(1, u16::arbitrary(g) as u64));
        }
        let genesis = Bound {
            time_ms: 0,
            slot: 0,
            epoch: 0,
        };
        let genesis_era_params = EraParams::arbitrary(g);

        let mut prev_bound = genesis;
        let mut prev_era_params = genesis_era_params;

        // For each boundary, compute the time and slot for that epoch based on the era params and
        // construct a summary from the boundary pair
        let mut summaries = vec![];
        for i in 1..g.size() {
            let boundary = boundaries[i];
            let epochs_elapsed = boundary - prev_bound.epoch;
            let slots_elapsed = epochs_elapsed * prev_era_params.epoch_size_slots;
            let time_elapsed = slots_elapsed * prev_era_params.slot_length;
            let new_bound = Bound {
                time_ms: prev_bound.time_ms + time_elapsed,
                slot: prev_bound.slot + slots_elapsed,
                epoch: boundary,
            };
            let new_era_params = EraParams::arbitrary(g);

            summaries.push(Summary {
                start: prev_bound,
                end: new_bound.clone(),
                params: prev_era_params,
            });

            prev_era_params = new_era_params;
            prev_bound = new_bound;
        }

        EraHistory {
            eras: summaries,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum TimeHorizonError {
    PastTimeHorizon,
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
                return Err(TimeHorizonError::InvalidEraHistory)
            }
            if era.end.slot >= slot {
                let slots_elapsed = slot - era.start.slot;
                let time_elapsed = era.params.slot_length * slots_elapsed;
                let relative_time = era.start.time_ms + time_elapsed;
                return Ok(relative_time)
            }
        }
        return Err(TimeHorizonError::PastTimeHorizon)
    }

    pub fn slot_to_absolute_time(&self, slot: u64, system_start: u64) -> Result<u64, TimeHorizonError> {
        self.slot_to_relative_time(slot).map(|t| system_start + t)
    }

    pub fn relative_time_to_slot(&self, time: u64) -> Result<u64, TimeHorizonError> {
        for era in &self.eras {
            if era.start.time_ms > time {
                return Err(TimeHorizonError::InvalidEraHistory)
            }
            if era.end.time_ms >= time {
                let time_elapsed = time - era.start.time_ms;
                let slots_elapsed = time_elapsed / era.params.slot_length;
                let slot = era.start.slot + slots_elapsed;
                return Ok(slot)
            }
        }
        return Err(TimeHorizonError::PastTimeHorizon)
    }

    pub fn slot_to_epoch(&self, slot: u64) -> Result<u64, TimeHorizonError> {
        for era in &self.eras {
            if era.start.slot > slot {
                return Err(TimeHorizonError::InvalidEraHistory)
            }
            if era.end.slot >= slot {
                let slots_elapsed = slot - era.start.slot;
                let epochs_elapsed = slots_elapsed / era.params.epoch_size_slots;
                let epoch_number = era.start.epoch + epochs_elapsed;
                return Ok(epoch_number)
            }
        }
        return Err(TimeHorizonError::PastTimeHorizon)
    }

    pub fn epoch_bounds(&self, epoch: u64) -> Result<EpochBounds, TimeHorizonError> {
        for era in &self.eras {
            if era.start.epoch > epoch {
                return Err(TimeHorizonError::InvalidEraHistory)
            }
            // We can't answer queries about the upper bound epoch of the era because the bound is
            // exclusive.
            if era.end.epoch > epoch {
                let epochs_elapsed = epoch - era.start.epoch;
                let offset = era.start.slot;
                let start = offset + era.params.epoch_size_slots * epochs_elapsed;
                let end = offset + era.params.epoch_size_slots * (epochs_elapsed + 1);
                return Ok(EpochBounds {
                    start: start,
                    end: end,
                })
            }
        }
        return Err(TimeHorizonError::PastTimeHorizon);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex;
    #[macro_use(quickcheck)]
    use quickcheck_macros::quickcheck;

    fn default_params() -> EraParams {
        EraParams::new(86400, 1000).unwrap()
    }

    fn one_era() -> EraHistory {
        EraHistory {
            eras: vec![
                Summary {
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
                },
            ],
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
        assert_eq!(eras.epoch_bounds(10), Err(TimeHorizonError::PastTimeHorizon));
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
    fn encode_era_history() {
        let eras = one_era();
        let buffer = minicbor::to_vec(&eras).unwrap();
        assert_eq!(
            hex::encode(buffer),
            "9f9f9f000000ff9f1a337f98001a000d2f000aff9f1a000151801903e8ffffff"
        );
    }

    #[quickcheck]
    fn roundtrip_era_history(eras: EraHistory) {
        let buffer = minicbor::to_vec(&eras).unwrap();
        let decoded = minicbor::decode(&buffer).unwrap();
        assert_eq!(eras, decoded);
    }
}
