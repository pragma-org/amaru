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

use amaru_kernel::{EraHistory, Slot, ValidityInterval};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum InvalidValidityInterval {
    #[error("current slot {slot} not within transaction validity interval {validity_interval}")]
    OutsideValidityInterval { slot: Slot, validity_interval: ValidityInterval },

    #[error("upper validity bound {0} is past the forecast horizon")]
    OutsideForecast(u64),
}

pub fn execute(
    validity_interval: ValidityInterval,
    // TODO: 'enforce_forecast_horizon' boolean
    //
    // This boolean really shouldn't exist. Instead, we should selectively construct a TimeRange
    // as needed by phase-2 validations when phase-2 validations are required (i.e. there are
    // scripts to execute).
    //
    // One way to possibly do this would be via returning a closure as result, that if invoked,
    // returns a TimeRange after having performed the additional horizon check.
    enforce_forecast_horizon: bool,
    era_history: &EraHistory,
    current_slot: Slot,
) -> Result<(), InvalidValidityInterval> {
    if !validity_interval.includes(current_slot) {
        return Err(InvalidValidityInterval::OutsideValidityInterval { slot: current_slot, validity_interval });
    }

    if enforce_forecast_horizon && let Some(upper_bound) = validity_interval.upper_bound() {
        era_history
            .slot_to_relative_time(upper_bound, current_slot)
            .map_err(|_| InvalidValidityInterval::OutsideForecast(upper_bound.as_u64()))?;
    }

    Ok(())
}
