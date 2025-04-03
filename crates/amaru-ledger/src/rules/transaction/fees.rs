// Copyright 2025 PRAGMA
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

use crate::context::{PotsSlice, UtxoSlice};
use amaru_kernel::{HasLovelace, Lovelace, MintedTransactionOutput, TransactionInput};

#[derive(Debug, thiserror::Error, Clone)]
pub enum InvalidFees {
    #[error("unknown collateral input at position {position}")]
    UnknownCollateralInput { position: usize },
}

pub(crate) fn execute<C>(
    context: &mut C,
    is_valid: bool,
    fees: Lovelace,
    collateral: Option<&Vec<TransactionInput>>,
    collateral_return: Option<&MintedTransactionOutput<'_>>,
) -> Result<(), InvalidFees>
where
    C: UtxoSlice + PotsSlice,
{
    if is_valid {
        context.add_fees(fees);
        return Ok(());
    }

    let total_collateral = collateral
        .map(|x| x.as_slice())
        .unwrap_or(&[])
        .iter()
        .enumerate()
        .try_fold(0, |total, (position, input)| {
            let output = context
                .lookup(input)
                .ok_or(InvalidFees::UnknownCollateralInput { position })?;

            Ok(total + output.lovelace())
        })?;

    let collateral_return = collateral_return.map(|o| o.lovelace()).unwrap_or_default();

    // FIXME: Check for underflow..
    context.add_fees(total_collateral - collateral_return);

    Ok(())
}
