#[derive(PartialEq, Eq)]
pub struct Bound {
    pub bound_time: u64,
    pub bound_slot: u64,
    pub bound_epoch: u64,
}

#[derive(PartialEq, Eq)]
pub enum EraEnd {
    Bounded(Bound),
    Unbounded,
}

#[derive(PartialEq, Eq)]
pub struct EraParams {
    pub era_epoch_size: u64,
    // Milliseconds
    pub era_slot_length: u64,
    // If the next era has not yet been announced then it is impossible for it
    // to occur in the next `era_safe_zone` slots after the tip. But because a
    // fork can only occur at an epoch boundary, in practice the safe zone
    // will get rounded up to the end of that epoch.
    pub era_safe_zone: u64,
    pub era_genesis_window: u64,
}

#[derive(PartialEq, Eq)]
pub struct Summary {
    pub era_start: Bound,
    pub era_end: EraEnd,
    pub era_params: EraParams,
}

// A complete history of eras that have taken place.
#[derive(PartialEq, Eq)]
pub struct EraHistory {
    pub eras: Vec<Summary>,
}

#[derive(PartialEq, Eq)]
pub enum TimeHorizonError {
    SlotIsPastTimeHorizon(u64),
    InvalidEraHistory(u64),
}

// The last era in the provided EraHistory must end at the time horizon for
// correct results. Returns number of seconds elapsed since the system start
// time.
pub fn slot_to_relative_time(eras: &EraHistory, slot: u64) -> Result<u64, TimeHorizonError> {
    for era in &eras.eras {
        if era.era_start.bound_slot > slot {
            return Err(TimeHorizonError::InvalidEraHistory(slot))
        }
        let in_era = match &era.era_end {
            EraEnd::Bounded(bound) => bound.bound_slot >= slot,
            EraEnd::Unbounded => true,
        };
        if in_era {
            let slots_elapsed = slot - era.era_start.bound_slot;
            let time_elapsed = era.era_params.era_slot_length * slots_elapsed;
            let relative_time = era.era_start.bound_time + time_elapsed;
            return Ok(relative_time)
        }
    }
    return Err(TimeHorizonError::SlotIsPastTimeHorizon(slot))
}

pub fn slot_to_absolute_time(eras: &EraHistory, slot: u64, system_start: u64) -> Result<u64, TimeHorizonError> {
    slot_to_relative_time(eras, slot).map(|t| system_start + t)
}

pub fn slot_to_epoch(eras: &EraHistory, slot: u64) -> Result<u64, TimeHorizonError> {
    for era in &eras.eras {
        if era.era_start.bound_slot > slot {
            return Err(TimeHorizonError::InvalidEraHistory(slot))
        }
        let in_era = match &era.era_end {
            EraEnd::Bounded(bound) => bound.bound_slot >= slot,
            EraEnd::Unbounded => true,
        };
        if in_era {
            let slots_elapsed = slot - era.era_start.bound_slot;
            let epochs_elapsed = slots_elapsed / era.era_params.era_epoch_size;
            let epoch_number = era.era_start.bound_epoch + epochs_elapsed;
            return Ok(epoch_number)
        }
    }
    return Err(TimeHorizonError::SlotIsPastTimeHorizon(slot))
}

pub struct EpochBounds {
    pub start_slot: u64,
    pub end_slot: u64,
}

pub fn epoch_bounds(eras: &EraHistory, epoch: u64) -> Result<EpochBounds, TimeHorizonError> {
    for era in &eras.eras {
        if era.era_start.bound_epoch > epoch {
            return Err(TimeHorizonError::InvalidEraHistory(epoch))
        }
        let in_era = match &era.era_end {
            // We can't answer queries about the upper bound epoch of the era because the bound is
            // exclusive.
            EraEnd::Bounded(bound) => bound.bound_epoch > epoch,
            EraEnd::Unbounded => true,
        };
        if in_era {
            let epochs_elapsed = epoch - era.era_start.bound_epoch;
            let offset = era.era_start.bound_slot;
            let start_slot = offset + era.era_params.era_epoch_size * epochs_elapsed;
            let end_slot = offset + era.era_params.era_epoch_size * (epochs_elapsed + 1);
            return Ok(EpochBounds {
                start_slot: start_slot,
                end_slot: end_slot,
            })
        }
    }
    return Err(TimeHorizonError::SlotIsPastTimeHorizon(epoch));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_slot_to_time() {
        let eras = EraHistory {
            eras: vec![
                Summary {
                    era_start: Bound {
                        bound_time: 0,
                        bound_slot: 0,
                        bound_epoch: 0,
                    },
                    era_end: EraEnd::Bounded(Bound {
                        bound_time: 86400000,
                        bound_slot: 86400,
                        bound_epoch: 1,
                    }),
                    era_params: EraParams {
                        era_epoch_size: 43200,
                        era_slot_length: 1000,
                        era_safe_zone: 129600, // 3k/f where k=2160 and f=0.05
                        era_genesis_window: 0,
                    },
                },
                Summary {
                    era_start: Bound {
                        bound_time: 86400000,
                        bound_slot: 86400,
                        bound_epoch: 1,
                    },
                    era_end: EraEnd::Bounded(Bound {
                        bound_time: 172800000,
                        bound_slot: 172800,
                        bound_epoch: 2,
                    }),
                    era_params: EraParams {
                        era_epoch_size: 43200,
                        era_slot_length: 1000,
                        era_safe_zone: 129600,
                        era_genesis_window: 0,
                    },
                },
            ],
        };
        let t0 = slot_to_relative_time(&eras, 172801);
        match t0 {
            Err(TimeHorizonError::SlotIsPastTimeHorizon(s)) => {
                assert_eq!(s, 172801);
            }
            _ => {
                panic!("expected error");
            }
        }
        let t1 = slot_to_relative_time(&eras, 172800);
        match t1 {
            Ok(t) => {
                assert_eq!(t, 172800000);
            }
            _ => {
                panic!("expected no error");
            }
        }
    }

    #[test]
    fn test_epoch_bounds() {
        let eras = EraHistory {
            eras: vec![
                Summary {
                    era_start: Bound {
                        bound_time: 0,
                        bound_slot: 0,
                        bound_epoch: 0,
                    },
                    era_end: EraEnd::Bounded(Bound {
                        bound_time: 864000000,
                        bound_slot: 864000,
                        bound_epoch: 10,
                    }),
                    era_params: EraParams {
                        era_epoch_size: 86400,
                        era_slot_length: 1000,
                        era_safe_zone: 129600, // 3k/f where k=2160 and f=0.05
                        era_genesis_window: 0,
                    },
                },
            ],
        };
        let bounds0 = epoch_bounds(&eras, 1);
        match bounds0 {
            Ok(e) => {
                assert_eq!(e.start_slot, 86400);
                assert_eq!(e.end_slot, 172800);
            }
            _ => {
                panic!("expected no error");
            }
        }
        let bounds1 = epoch_bounds(&eras, 10);
        match bounds1 {
            Err(TimeHorizonError::SlotIsPastTimeHorizon(s)) => {
                assert_eq!(s, 10);
            }
            _ => {
                panic!("expected error");
            }
        }
    }

    #[test]
    fn test_slot_to_epoch() {
        let eras = EraHistory {
            eras: vec![
                Summary {
                    era_start: Bound {
                        bound_time: 0,
                        bound_slot: 0,
                        bound_epoch: 0,
                    },
                    era_end: EraEnd::Bounded(Bound {
                        bound_time: 864000000,
                        bound_slot: 864000,
                        bound_epoch: 10,
                    }),
                    era_params: EraParams {
                        era_epoch_size: 86400,
                        era_slot_length: 1000,
                        era_safe_zone: 129600, // 3k/f where k=2160 and f=0.05
                        era_genesis_window: 0,
                    },
                },
            ],
        };
        let e0 = slot_to_epoch(&eras, 0);
        match e0 {
            Ok(e) => {
                assert_eq!(e, 0);
            }
            _ => {
                panic!("expected no error");
            }
        }
        let e1 = slot_to_epoch(&eras, 86399);
        match e1 {
            Ok(e) => {
                assert_eq!(e, 0);
            }
            _ => {
                panic!("expected no error");
            }
        }
        let e2 = slot_to_epoch(&eras, 864000);
        match e2 {
            Ok(e) => {
                assert_eq!(e, 10);
            }
            _ => {
                panic!("expected no error");
            }
        }
        let e3 = slot_to_epoch(&eras, 864001);
        match e3 {
            Err(TimeHorizonError::SlotIsPastTimeHorizon(s)) => {
                assert_eq!(s, 864001);
            }
            _ => {
                panic!("expected error");
            }
        }
    }
}
