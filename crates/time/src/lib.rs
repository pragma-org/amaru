use minicbor::{Encode, Decode};

type Slot = u64;

#[derive(Clone, PartialEq, Eq)]
pub struct Bound {
    pub time: u64, // Milliseconds
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
        self.time.encode(e, ctx)?;
        self.slot.encode(e, ctx)?;
        self.epoch.encode(e, ctx)?;
        e.end()?;
        Ok(())
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct EraParams {
    epoch_size: u64,
    slot_length: u64, // Milliseconds
}

impl EraParams {
    pub fn new(epoch_size: u64, slot_length: u64) -> Option<Self> {
        if epoch_size == 0 {
            return None;
        }
        if slot_length == 0 {
            return None;
        }
        Some(EraParams {
            epoch_size,
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
        self.epoch_size.encode(e, ctx)?;
        self.slot_length.encode(e, ctx)?;
        e.end()?;
        Ok(())
    }
}

// The start is inclusive and the end is exclusive. In a valid EraHistory, the
// end of each era will equal the start of the next one.
#[derive(Clone, PartialEq, Eq)]
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

// A complete history of eras that have taken place.
#[derive(PartialEq, Eq)]
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
                let relative_time = era.start.time + time_elapsed;
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
            if era.start.time > time {
                return Err(TimeHorizonError::InvalidEraHistory)
            }
            if era.end.time >= time {
                let time_elapsed = time - era.start.time;
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
                let epochs_elapsed = slots_elapsed / era.params.epoch_size;
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
                let start = offset + era.params.epoch_size * epochs_elapsed;
                let end = offset + era.params.epoch_size * (epochs_elapsed + 1);
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

    #[test]
    fn test_slot_to_time() {
        let params = EraParams::new(86400, 1000).unwrap();
        let eras = EraHistory {
            eras: vec![
                Summary {
                    start: Bound {
                        time: 0,
                        slot: 0,
                        epoch: 0,
                    },
                    end: Bound {
                        time: 86400000,
                        slot: 86400,
                        epoch: 1,
                    },
                    params: params.clone(),
                },
                Summary {
                    start: Bound {
                        time: 86400000,
                        slot: 86400,
                        epoch: 1,
                    },
                    end: Bound {
                        time: 172800000,
                        slot: 172800,
                        epoch: 2,
                    },
                    params: params.clone(),
                },
            ],
        };
        let t0 = eras.slot_to_relative_time(172801);
        assert_eq!(t0, Err(TimeHorizonError::PastTimeHorizon));
        let t1 = eras.slot_to_relative_time(172800);
        assert_eq!(t1, Ok(172800000));
    }

    #[test]
    fn test_epoch_bounds() {
        let params = EraParams::new(86400, 1000).unwrap();
        let eras = EraHistory {
            eras: vec![
                Summary {
                    start: Bound {
                        time: 0,
                        slot: 0,
                        epoch: 0,
                    },
                    end: Bound {
                        time: 864000000,
                        slot: 864000,
                        epoch: 10,
                    },
                    params: params,
                },
            ],
        };
        assert_eq!(eras.epoch_bounds(1).unwrap().start, 86400);
        assert_eq!(eras.epoch_bounds(1).unwrap().end, 172800);
        assert_eq!(eras.epoch_bounds(10), Err(TimeHorizonError::PastTimeHorizon));
    }

    #[test]
    fn test_slot_to_epoch() {
        let params = EraParams::new(86400, 1000).unwrap();
        let eras = EraHistory {
            eras: vec![
                Summary {
                    start: Bound {
                        time: 0,
                        slot: 0,
                        epoch: 0,
                    },
                    end: Bound {
                        time: 864000000,
                        slot: 864000,
                        epoch: 10,
                    },
                    params: params,
                },
            ],
        };
        let e0 = eras.slot_to_epoch(0);
        assert_eq!(e0, Ok(0));
        let e1 = eras.slot_to_epoch(86399);
        assert_eq!(e1, Ok(0));
        let e2 = eras.slot_to_epoch(864000);
        assert_eq!(e2, Ok(10));
        let e3 = eras.slot_to_epoch(864001);
        assert_eq!(e3, Err(TimeHorizonError::PastTimeHorizon));
    }

    #[test]
    fn test_encode_era_history() {
        let params = EraParams::new(86400, 1000).unwrap();
        let eras = EraHistory {
            eras: vec![
                Summary {
                    start: Bound {
                        time: 0,
                        slot: 0,
                        epoch: 0,
                    },
                    end: Bound {
                        time: 864000000,
                        slot: 864000,
                        epoch: 10,
                    },
                    params: params,
                },
            ],
        };
        let buffer = minicbor::to_vec(&eras).unwrap();
        assert_eq!(
            hex::encode(buffer),
            "9f9f9f000000ff9f1a337f98001a000d2f000aff9f1a000151801903e8ffffff"
        );
    }
}
