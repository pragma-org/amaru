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

#[derive(Debug, thiserror::Error)]
pub enum InvalidFees {
    #[error("unknown collateral input at position {position}")]
    UnknownCollateralInput { position: usize },
    #[error("collateral return value {{total_collateral_return}} is greater than total collateral input {{total_collateral_input}}")]
    CollateralReturnOverflow {
        total_collateral_input: u64,
        total_collateral_return: u64,
    },
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

    if total_collateral < collateral_return {
        return Err(InvalidFees::CollateralReturnOverflow {
            total_collateral_input: total_collateral,
            total_collateral_return: collateral_return,
        });
    }
    context.add_fees(total_collateral - collateral_return);

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::{
        context::assert::{AssertPreparationContext, AssertValidationContext},
        rules::tests::fixture_context,
    };
    use amaru_kernel::{include_cbor, include_json, json, KeepRaw, MintedTransactionBody};
    use amaru_tracing_json::assert_trace;
    use test_case::test_case;

    use super::InvalidFees;

    macro_rules! fixture {
        ($hash:literal, $is_valid:expr) => {
            (
                fixture_context!($hash),
                include_cbor!(concat!("transactions/preprod/", $hash, "/tx.cbor")),
                include_json!(concat!("transactions/preprod/", $hash, "/expected.traces")),
                $is_valid,
            )
        };
        ($hash:literal, $variant:literal, $is_valid:expr) => {
            (
                fixture_context!($hash, $variant),
                include_cbor!(concat!(
                    "transactions/preprod/",
                    $hash,
                    "/",
                    $variant,
                    "/tx.cbor"
                )),
                include_json!(concat!(
                    "transactions/preprod/",
                    $hash,
                    "/",
                    $variant,
                    "/expected.traces"
                )),
                $is_valid,
            )
        };
    }

    #[test_case(fixture!("efecb8d07a7c15e80c1daf3a25a3b89728506ddad4e18cd9c9512cea44805b4f", true); "Valid transaction")]
    #[test_case(fixture!("efecb8d07a7c15e80c1daf3a25a3b89728506ddad4e18cd9c9512cea44805b4f", "invalid-transaction", false); "Invalid transaction")]
    #[test_case(fixture!("efecb8d07a7c15e80c1daf3a25a3b89728506ddad4e18cd9c9512cea44805b4f", "collateral-underflow", false) =>
        matches Err(InvalidFees::CollateralReturnOverflow { total_collateral_input, total_collateral_return }) if total_collateral_input == 5000000 && total_collateral_return == 10000000;
        "Collateral overflow")]
    #[test_case(fixture!("efecb8d07a7c15e80c1daf3a25a3b89728506ddad4e18cd9c9512cea44805b4f", "invalid-collateral", false) =>
        matches Err(InvalidFees::UnknownCollateralInput { position }) if position == 0;
        "Unresolved collateral")]
    fn fees(
        (ctx, tx, expected_traces, is_valid): (
            AssertPreparationContext,
            KeepRaw<'_, MintedTransactionBody<'_>>,
            Vec<json::Value>,
            bool,
        ),
    ) -> Result<(), InvalidFees> {
        assert_trace(
            || {
                let mut validation_context = AssertValidationContext::from(ctx.clone());
                super::execute(
                    &mut validation_context,
                    is_valid,
                    tx.fee,
                    tx.collateral.as_deref(),
                    tx.collateral_return.as_ref(),
                )
            },
            expected_traces,
        )
    }
}
