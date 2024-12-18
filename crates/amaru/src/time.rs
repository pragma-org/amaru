#[derive(PartialEq, Eq)]
pub struct Bound {
    pub bound_time: u64, // Milliseconds
    pub bound_slot: u64,
    pub bound_epoch: u64,
}

#[derive(PartialEq, Eq)]
pub struct EraParams {
    pub epoch_size: u64,
    pub slot_length: u64, // Milliseconds
    // If the next era has not yet been announced then it is impossible for it
    // to occur in the next `safe_zone` slots after the tip. But because a
    // fork can only occur at an epoch boundary, in practice the safe zone
    // will get rounded up to the end of that epoch.
    pub safe_zone: u64,
    pub genesis_window: u64,
}

// The start is inclusive and the end is exclusive. In a valid EraHistory, the
// end of each era will equal the start of the next one.
#[derive(PartialEq, Eq)]
pub struct Summary {
    pub start: Bound,
    pub end: Bound,
    pub params: EraParams,
}

// A complete history of eras that have taken place.
#[derive(PartialEq, Eq)]
pub struct EraHistory {
    pub eras: Vec<Summary>,
}

#[derive(PartialEq, Eq)]
pub enum TimeHorizonError {
    PastTimeHorizon,
    InvalidEraHistory,
}

// The last era in the provided EraHistory must end at the time horizon for accurate results. The
// horizon is the end of the epoch containing the end of the current era's safe zone relative to
// the current tip. Returns number of milliseconds elapsed since the system start time.
pub fn slot_to_relative_time(eras: &EraHistory, slot: u64) -> Result<u64, TimeHorizonError> {
    for era in &eras.eras {
        if era.start.bound_slot > slot {
            return Err(TimeHorizonError::InvalidEraHistory)
        }
        if era.end.bound_slot >= slot {
            let slots_elapsed = slot - era.start.bound_slot;
            let time_elapsed = era.params.slot_length * slots_elapsed;
            let relative_time = era.start.bound_time + time_elapsed;
            return Ok(relative_time)
        }
    }
    return Err(TimeHorizonError::PastTimeHorizon)
}

pub fn slot_to_absolute_time(eras: &EraHistory, slot: u64, system_start: u64) -> Result<u64, TimeHorizonError> {
    slot_to_relative_time(eras, slot).map(|t| system_start + t)
}

pub fn relative_time_to_slot(eras: &EraHistory, time: u64) -> Result<u64, TimeHorizonError> {
    for era in &eras.eras {
        if era.start.bound_time > time {
            return Err(TimeHorizonError::InvalidEraHistory)
        }
        if era.end.bound_time >= time {
            let time_elapsed = time - era.start.bound_time;
            let slots_elapsed = time_elapsed / era.params.slot_length;
            let slot = era.start.bound_slot + slots_elapsed;
            return Ok(slot)
        }
    }
    return Err(TimeHorizonError::PastTimeHorizon)
}

pub fn slot_to_epoch(eras: &EraHistory, slot: u64) -> Result<u64, TimeHorizonError> {
    for era in &eras.eras {
        if era.start.bound_slot > slot {
            return Err(TimeHorizonError::InvalidEraHistory)
        }
        if era.end.bound_slot >= slot {
            let slots_elapsed = slot - era.start.bound_slot;
            let epochs_elapsed = slots_elapsed / era.params.epoch_size;
            let epoch_number = era.start.bound_epoch + epochs_elapsed;
            return Ok(epoch_number)
        }
    }
    return Err(TimeHorizonError::PastTimeHorizon)
}

pub struct EpochBounds {
    pub start_slot: u64,
    pub end_slot: u64,
}

pub fn epoch_bounds(eras: &EraHistory, epoch: u64) -> Result<EpochBounds, TimeHorizonError> {
    for era in &eras.eras {
        if era.start.bound_epoch > epoch {
            return Err(TimeHorizonError::InvalidEraHistory)
        }
            // We can't answer queries about the upper bound epoch of the era because the bound is
            // exclusive.
        if era.end.bound_epoch > epoch {
            let epochs_elapsed = epoch - era.start.bound_epoch;
            let offset = era.start.bound_slot;
            let start_slot = offset + era.params.epoch_size * epochs_elapsed;
            let end_slot = offset + era.params.epoch_size * (epochs_elapsed + 1);
            return Ok(EpochBounds {
                start_slot: start_slot,
                end_slot: end_slot,
            })
        }
    }
    return Err(TimeHorizonError::PastTimeHorizon);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_slot_to_time() {
        let eras = EraHistory {
            eras: vec![
                Summary {
                    start: Bound {
                        bound_time: 0,
                        bound_slot: 0,
                        bound_epoch: 0,
                    },
                    end: Bound {
                        bound_time: 86400000,
                        bound_slot: 86400,
                        bound_epoch: 1,
                    },
                    params: EraParams {
                        epoch_size: 86400,
                        slot_length: 1000,
                        safe_zone: 25920, // 3k/f where k=432 and f=0.05
                        genesis_window: 25920,
                    },
                },
                Summary {
                    start: Bound {
                        bound_time: 86400000,
                        bound_slot: 86400,
                        bound_epoch: 1,
                    },
                    end: Bound {
                        bound_time: 172800000,
                        bound_slot: 172800,
                        bound_epoch: 2,
                    },
                    params: EraParams {
                        epoch_size: 86400,
                        slot_length: 1000,
                        safe_zone: 25920,
                        genesis_window: 25920,
                    },
                },
            ],
        };
        let t0 = slot_to_relative_time(&eras, 172801);
        match t0 {
            Err(TimeHorizonError::PastTimeHorizon) => {
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
                    start: Bound {
                        bound_time: 0,
                        bound_slot: 0,
                        bound_epoch: 0,
                    },
                    end: Bound {
                        bound_time: 864000000,
                        bound_slot: 864000,
                        bound_epoch: 10,
                    },
                    params: EraParams {
                        epoch_size: 86400,
                        slot_length: 1000,
                        safe_zone: 25920,
                        genesis_window: 25920,
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
            Err(TimeHorizonError::PastTimeHorizon) => {
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
                    start: Bound {
                        bound_time: 0,
                        bound_slot: 0,
                        bound_epoch: 0,
                    },
                    end: Bound {
                        bound_time: 864000000,
                        bound_slot: 864000,
                        bound_epoch: 10,
                    },
                    params: EraParams {
                        epoch_size: 86400,
                        slot_length: 1000,
                        safe_zone: 25920,
                        genesis_window: 25920,
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
            Err(TimeHorizonError::PastTimeHorizon) => {
                assert_eq!(s, 864001);
            }
            _ => {
                panic!("expected error");
            }
        }
    }
}
