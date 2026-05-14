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

use amaru_kernel::{
    ExUnitPrices, ExUnits, HasExUnits, HasLovelace, Lovelace, MemoizedTransactionOutput, ProtocolParameters,
    RationalNumber, TransactionInput, WitnessSet,
};

use crate::context::{PotsSlice, UtxoSlice};

#[derive(Debug, thiserror::Error)]
pub enum InvalidFees {
    #[error("unknown collateral input at position {position}")]
    UnknownCollateralInput { position: usize },
    #[error(
        "collateral return value {{total_collateral_return}} is greater than total collateral input {{total_collateral_input}}"
    )]
    CollateralReturnOverflow { total_collateral_input: u64, total_collateral_return: u64 },
    #[error("declared fee {provided} below minimum {minimum}")]
    FeeTooSmall { provided: Lovelace, minimum: Lovelace },
}

/// Validates declared fee against the computed minimum, and then
/// performs fee/collateral collection into the context pots.
///
/// `ref_scripts_size` is the total on-wire byte size of every script attached
/// to the transaction's reference-input UTxO entries; already accumulated by
/// [`crate::rules::transaction::phase_one::inputs::execute`].
///
/// Reference: <https://github.com/IntersectMBO/cardano-ledger/blob/0cfbf861cfb456660a7b73281c6fb714a53d40f9/eras/conway/impl/src/Cardano/Ledger/Conway/Tx.hs#L89>
#[expect(clippy::too_many_arguments)]
pub(crate) fn execute<C>(
    context: &mut C,
    is_valid: bool,
    fees: Lovelace,
    tx_size: u64,
    witness_set: &WitnessSet,
    ref_scripts_size: u64,
    pp: &ProtocolParameters,
    collateral: Option<&[TransactionInput]>,
    collateral_return: Option<&MemoizedTransactionOutput>,
) -> Result<(), InvalidFees>
where
    C: UtxoSlice + PotsSlice,
{
    let minimum = compute_min_fee(tx_size, witness_set, ref_scripts_size, pp);
    if fees < minimum {
        return Err(InvalidFees::FeeTooSmall { provided: fees, minimum });
    }

    if is_valid {
        context.add_fees(fees);
        return Ok(());
    }

    let total_collateral = collateral.unwrap_or(&[]).iter().enumerate().try_fold(0, |total, (position, input)| {
        let output = context.lookup(input).ok_or(InvalidFees::UnknownCollateralInput { position })?;

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

fn compute_min_fee(tx_size: u64, witness_set: &WitnessSet, ref_scripts_size: u64, pp: &ProtocolParameters) -> Lovelace {
    let linear = pp.min_fee_a.saturating_mul(tx_size).saturating_add(pp.min_fee_b);
    let plutus = plutus_exec_fee(witness_set.total_ex_units(), &pp.prices);
    let tiered = tier_ref_script_fee(
        ref_scripts_size,
        u64::from(pp.ref_script_cost_stride),
        &pp.min_fee_ref_script_lovelace_per_byte,
        &pp.ref_script_cost_multiplier,
    );
    linear.saturating_add(plutus).saturating_add(tiered)
}

/// Plutus execution cost.
///
/// `ceiling( mem * mem_price + steps * step_price )`
///
/// Reference: <https://github.com/IntersectMBO/cardano-ledger/blob/0cfbf861cfb456660a7b73281c6fb714a53d40f9/libs/cardano-ledger-core/src/Cardano/Ledger/Plutus/ExUnits.hs#L182>
fn plutus_exec_fee(units: ExUnits, prices: &ExUnitPrices) -> Lovelace {
    let mem_den = prices.mem_price.denominator;
    let step_den = prices.step_price.denominator;
    if mem_den == 0 || step_den == 0 {
        return 0;
    }

    let mem_num = units.mem.saturating_mul(prices.mem_price.numerator);
    let step_num = units.steps.saturating_mul(prices.step_price.numerator);

    let denom = mem_den.saturating_mul(step_den);
    if denom == 0 {
        return 0;
    }

    let total_num = mem_num.saturating_mul(step_den).saturating_add(step_num.saturating_mul(mem_den));
    total_num.saturating_add(denom - 1) / denom
}

/// Tiered reference-script fee.
///
/// Accumulates `sum(chunk_i * rate_i)` across every full
/// tier and the trailing partial tier, then applies `floor` exactly once at
/// the end. `rate_i` is multiplied by `multiplier` between tiers.
///
/// Reference: <https://github.com/IntersectMBO/cardano-ledger/blob/0cfbf861cfb456660a7b73281c6fb714a53d40f9/eras/conway/impl/src/Cardano/Ledger/Conway/Tx.hs#L111>
fn tier_ref_script_fee(size: u64, stride: u64, base_rate: &RationalNumber, multiplier: &RationalNumber) -> Lovelace {
    if size == 0 || stride == 0 || base_rate.denominator == 0 || multiplier.denominator == 0 {
        return 0;
    }

    let (acc_num, acc_den) = sum_tiers(
        size,
        stride,
        (base_rate.numerator, base_rate.denominator),
        (multiplier.numerator, multiplier.denominator),
        (0, 1),
    );

    if acc_den == 0 {
        return 0;
    }
    acc_num / acc_den
}

fn sum_tiers(remaining: u64, stride: u64, rate: (u64, u64), mult: (u64, u64), acc: (u64, u64)) -> (u64, u64) {
    let (rate_num, rate_den) = rate;
    if remaining < stride {
        if remaining == 0 {
            return acc;
        }
        let term = (remaining.saturating_mul(rate_num), rate_den);
        return rational_add(acc, term);
    }

    let term = (stride.saturating_mul(rate_num), rate_den);
    let acc = rational_add(acc, term);

    let (mult_num, mult_den) = mult;
    let next_num = rate_num.saturating_mul(mult_num);
    let next_den = rate_den.saturating_mul(mult_den);
    let g = gcd(next_num, next_den);
    let next_rate = if g > 1 { (next_num / g, next_den / g) } else { (next_num, next_den) };

    sum_tiers(remaining - stride, stride, next_rate, mult, acc)
}

/// `a/b + c/d`, reduced by GCD.
fn rational_add(a: (u64, u64), b: (u64, u64)) -> (u64, u64) {
    let (a_num, a_den) = a;
    let (b_num, b_den) = b;
    let num = a_num.saturating_mul(b_den).saturating_add(b_num.saturating_mul(a_den));
    let den = a_den.saturating_mul(b_den);
    let g = gcd(num, den);
    if g > 1 { (num / g, den / g) } else { (num, den) }
}

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 { a } else { gcd(b, a % b) }
}

#[cfg(test)]
mod tests {
    use amaru_kernel::{
        PREPROD_DEFAULT_PROTOCOL_PARAMETERS, ProtocolParameters, TransactionBody, WitnessSet, include_cbor,
        include_json, json,
    };
    use amaru_tracing_json::assert_trace;
    use test_case::test_case;

    use super::InvalidFees;
    use crate::{
        context::assert::{AssertPreparationContext, AssertValidationContext},
        rules::tests::fixture_context,
    };

    /// Protocol params where the min-fee floor is zero, so collateral tests can
    /// keep using arbitrary `tx.fee` without tripping `FeeTooSmall`.
    fn zero_min_fee_pp() -> ProtocolParameters {
        ProtocolParameters {
            min_fee_a: 0,
            min_fee_b: 0,
            min_fee_ref_script_lovelace_per_byte: amaru_kernel::RationalNumber { numerator: 0, denominator: 1 },
            ..PREPROD_DEFAULT_PROTOCOL_PARAMETERS.clone()
        }
    }

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
                include_cbor!(concat!("transactions/preprod/", $hash, "/", $variant, "/tx.cbor")),
                include_json!(concat!("transactions/preprod/", $hash, "/", $variant, "/expected.traces")),
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
        (ctx, tx, expected_traces, is_valid): (AssertPreparationContext, TransactionBody, Vec<json::Value>, bool),
    ) -> Result<(), InvalidFees> {
        let pp = zero_min_fee_pp();
        let witness_set = WitnessSet::default();
        assert_trace(
            || {
                let mut validation_context = AssertValidationContext::from(ctx.clone());
                super::execute(
                    &mut validation_context,
                    is_valid,
                    tx.fee,
                    /* tx_size */ 0,
                    &witness_set,
                    /* ref_scripts_size */ 0,
                    &pp,
                    tx.collateral.as_deref(),
                    tx.collateral_return.as_ref(),
                )
            },
            expected_traces,
        )
    }

    // tx_size = 300 with min_fee_a = 44, min_fee_b = 155_381 => minimum = 168_581.
    fn min_fee_pp() -> ProtocolParameters {
        ProtocolParameters { min_fee_a: 44, min_fee_b: 155_381, ..PREPROD_DEFAULT_PROTOCOL_PARAMETERS.clone() }
    }

    fn empty_context() -> AssertValidationContext {
        AssertValidationContext::from(AssertPreparationContext { utxo: std::collections::BTreeMap::new() })
    }

    #[test_case(1_000, 25_600, 15, 1, 12, 10 => 15_000; "tier: within first tier (1000 bytes * 15)")]
    #[test_case(250, 100, 10, 1, 2, 1 => 5_000; "tier: spans 3 tiers (100*10 + 100*20 + 50*40)")]
    #[test_case(5, 5, 1, 3, 1, 1 => 1; "tier: final floor (floor(5/3) = 1)")]
    #[test_case(10, 5, 1, 3, 1, 1 => 3; "tier: rational accumulates across tiers (floor(5/3 + 5/3) = 3, not 2+2)")]
    fn tier_ref_script_fee(size: u64, stride: u64, base_n: u64, base_d: u64, mult_n: u64, mult_d: u64) -> u64 {
        super::tier_ref_script_fee(
            size,
            stride,
            &amaru_kernel::RationalNumber { numerator: base_n, denominator: base_d },
            &amaru_kernel::RationalNumber { numerator: mult_n, denominator: mult_d },
        )
    }

    #[test_case(0, 0, 5, 1, 7, 1 => 0; "plutus: zero units")]
    #[test_case(1_000, 0, 5, 1, 7, 1 => 5_000; "plutus: mem axis only (1000 * 5/1)")]
    #[test_case(0, 1_000, 5, 1, 7, 1 => 7_000; "plutus: steps axis only (1000 * 7/1)")]
    #[test_case(1, 1, 1, 3, 1, 3 => 1; "plutus: ceiling on fractional sum (1/3 + 1/3 = 2/3, ceil = 1)")]
    #[test_case(10, 10, 1, 3, 1, 4 => 6; "plutus: different denominators (10/3 + 10/4 = 70/12, ceil = 6)")]
    fn plutus_exec_fee(mem: u64, steps: u64, mem_n: u64, mem_d: u64, step_n: u64, step_d: u64) -> u64 {
        super::plutus_exec_fee(
            amaru_kernel::ExUnits { mem, steps },
            &amaru_kernel::ExUnitPrices {
                mem_price: amaru_kernel::RationalNumber { numerator: mem_n, denominator: mem_d },
                step_price: amaru_kernel::RationalNumber { numerator: step_n, denominator: step_d },
            },
        )
    }

    #[test_case(100_000 => matches Err(InvalidFees::FeeTooSmall { minimum: 168_581, .. }); "min fee: below minimum")]
    #[test_case(168_581 => matches Ok(()); "min fee: at minimum")]
    #[test_case(200_000 => matches Ok(()); "min fee: above minimum")]
    fn execute_min_fee(declared_fee: amaru_kernel::Lovelace) -> Result<(), InvalidFees> {
        let pp = min_fee_pp();
        let witness_set = WitnessSet::default();
        super::execute(&mut empty_context(), true, declared_fee, 300, &witness_set, 0, &pp, None, None)
    }
}
