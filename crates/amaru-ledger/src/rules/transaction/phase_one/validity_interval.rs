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

use amaru_kernel::{EraHistory, Slot, TransactionBody, WitnessSet};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum InvalidValidityInterval {
    #[error(
        "current slot {slot} not within transaction validity interval [{}, {})",
        invalid_before.map(|s| s.to_string()).unwrap_or_else(|| "-inf".into()),
        invalid_after.map(|s| s.to_string()).unwrap_or_else(|| "inf".into())
    )]
    OutsideValidityInterval { slot: Slot, invalid_before: Option<u64>, invalid_after: Option<u64> },

    #[error("upper validity bound {0} is past the forecast horizon")]
    OutsideForecast(u64),
}

pub fn execute(
    transaction_body: &TransactionBody,
    transaction_witness_set: &WitnessSet,
    era_history: &EraHistory,
    current_slot: Slot,
) -> Result<(), InvalidValidityInterval> {
    let invalid_before = transaction_body.validity_interval_start;
    let invalid_after = transaction_body.validity_interval_end;

    let slot_u64 = u64::from(current_slot);
    let in_interval = match (invalid_before, invalid_after) {
        (None, None) => true,
        (None, Some(ia)) => slot_u64 < ia,
        (Some(ib), None) => slot_u64 >= ib,
        (Some(ib), Some(ia)) => slot_u64 >= ib && slot_u64 < ia,
    };
    if !in_interval {
        return Err(InvalidValidityInterval::OutsideValidityInterval {
            slot: current_slot,
            invalid_before,
            invalid_after,
        });
    }

    if transaction_witness_set.redeemer.is_some()
        && let Some(ia) = invalid_after
    {
        era_history
            .slot_to_relative_time(Slot::from(ia), current_slot)
            .map_err(|_| InvalidValidityInterval::OutsideForecast(ia))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use amaru_kernel::{PREPROD_ERA_HISTORY, Slot, TransactionBody, WitnessSet, include_cbor};
    use test_case::test_case;

    use super::{InvalidValidityInterval, execute};

    macro_rules! fixture {
        ($hash:literal, $slot:expr) => {
            (
                include_cbor!(concat!("transactions/preprod/", $hash, "/tx.cbor")),
                include_cbor!(concat!("transactions/preprod/", $hash, "/witness.cbor")),
                Slot::from($slot as u64),
            )
        };
        ($hash:literal, $variant:literal, $slot:expr) => {
            (
                include_cbor!(concat!("transactions/preprod/", $hash, "/", $variant, "/tx.cbor")),
                include_cbor!(concat!("transactions/preprod/", $hash, "/witness.cbor")),
                Slot::from($slot as u64),
            )
        };
    }

    #[test_case(fixture!("3b54f084af170b30565b1befe25860214a690a6c7a310e2902504dbc609c318e", 91553500u64);
        "both bounds, slot inside")]
    #[test_case(fixture!("49e6100c24938acb075f3415ddd989c7e91a5c52b8eb848364c660577e11594a", 70147900u64);
        "end only, slot below end")]
    #[test_case(fixture!("0c22edee0ffd7c8f32d2fe4da1f144e9ef78dfb51e1678d5198493a83d6cf8ec", 70000000u64);
        "no bounds always valid")]
    #[test_case(fixture!("3b54f084af170b30565b1befe25860214a690a6c7a310e2902504dbc609c318e", 91553400u64) =>
        matches Err(InvalidValidityInterval::OutsideValidityInterval { .. });
        "slot before invalid_before")]
    #[test_case(fixture!("3b54f084af170b30565b1befe25860214a690a6c7a310e2902504dbc609c318e", 91553634u64) =>
        matches Err(InvalidValidityInterval::OutsideValidityInterval { .. });
        "slot at invalid_after (exclusive)")]
    #[test_case(fixture!("49e6100c24938acb075f3415ddd989c7e91a5c52b8eb848364c660577e11594a", 70147961u64) =>
        matches Err(InvalidValidityInterval::OutsideValidityInterval { .. });
        "end only, slot at end")]
    #[test_case(fixture!("3b54f084af170b30565b1befe25860214a690a6c7a310e2902504dbc609c318e", "far-future-end", 91553500u64) =>
        matches Err(InvalidValidityInterval::OutsideForecast(_));
        "redeemer with end past forecast horizon")]
    fn test_validity_interval(
        (transaction_body, witness_set, current_slot): (TransactionBody, WitnessSet, Slot),
    ) -> Result<(), InvalidValidityInterval> {
        execute(&transaction_body, &witness_set, &PREPROD_ERA_HISTORY, current_slot)
    }
}
