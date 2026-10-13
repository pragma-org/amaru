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

use amaru_kernel::Lovelace;
use thiserror::Error;

use crate::context::{BalanceSlice, PotsSliceMut};

#[derive(Debug, Error)]
pub enum InvalidTreasury {
    #[error("current treasury value mismatch: provided {provided}, expected {expected}")]
    ValueMismatch { provided: Lovelace, expected: Lovelace },
}

/// Counts the donation towards value preservation. When the transaction is valid, the declared treasury value must
/// match the current treasury and the donation is credited to the treasury. When it is invalid, neither happens.
pub(crate) fn execute<C>(
    context: &mut C,
    is_valid: bool,
    treasury_value: Option<Lovelace>,
    donation: Option<Lovelace>,
) -> Result<(), InvalidTreasury>
where
    C: PotsSliceMut + BalanceSlice,
{
    if is_valid && let Some(provided) = treasury_value {
        let expected = context.treasury();
        if provided != expected {
            return Err(InvalidTreasury::ValueMismatch { provided, expected });
        }
    }

    if let Some(donation) = donation {
        context.produce_lovelace(donation);
        if is_valid {
            context.add_donation(donation);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use test_case::test_case;

    use super::*;
    use crate::{context::DefaultValidationContext, state::volatile::VolatileFragment};

    #[test_case(true, None => matches Ok((1_000_000, -1_000_000)); "valid transaction donates")]
    #[test_case(false, None => matches Ok((0, -1_000_000)); "invalid transaction only balances its donation")]
    #[test_case(true, Some(1) => matches Err(InvalidTreasury::ValueMismatch { provided: 1, expected: 0 }); "valid transaction checks the treasury value")]
    #[test_case(false, Some(1) => matches Ok(_); "invalid transaction skips the treasury value")]
    fn donation(is_valid: bool, treasury_value: Option<Lovelace>) -> Result<(Lovelace, i128), InvalidTreasury> {
        let mut context = DefaultValidationContext::default();

        execute(&mut context, is_valid, treasury_value, Some(1_000_000))?;

        let balance = context.balance().coin();
        Ok((VolatileFragment::from(context).donations, balance))
    }
}
