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

use num::{BigUint, Zero};

use crate::{
    ExUnitPrices, ExUnits, HasExUnits, Lovelace, ProtocolParameters, RationalNumber, WitnessSet,
    rational_number::{SafeRatio, floor_to_lovelace, into_safe_ratio, safe_ratio},
};

pub fn minimum_fee(tx_size: u64, witness_set: &WitnessSet, ref_scripts_size: u64, pp: &ProtocolParameters) -> Lovelace {
    let size = size_fee(tx_size, pp.min_fee_a, pp.min_fee_b);

    let plutus = plutus_execution_fee(witness_set.total_ex_units(), &pp.prices);

    let ref_scripts = reference_scripts_fee(
        ref_scripts_size,
        u64::from(pp.ref_script_cost_stride),
        &pp.min_fee_ref_script_lovelace_per_byte,
        &pp.ref_script_cost_multiplier,
    );

    size + plutus + ref_scripts
}

pub fn size_fee(size: u64, per_byte_coefficient: u64, fixed_fee: u64) -> Lovelace {
    per_byte_coefficient * size + fixed_fee
}

/// Plutus execution cost: `ceil(mem * mem_price + steps * step_price)`.
///
/// Reference: <https://github.com/IntersectMBO/cardano-ledger/blob/0cfbf861cfb456660a7b73281c6fb714a53d40f9/libs/cardano-ledger-core/src/Cardano/Ledger/Plutus/ExUnits.hs#L182>
pub fn plutus_execution_fee(units: ExUnits, prices: &ExUnitPrices) -> Lovelace {
    let mem = safe_ratio(units.mem, 1) * into_safe_ratio(&prices.mem_price);
    let steps = safe_ratio(units.steps, 1) * into_safe_ratio(&prices.step_price);
    floor_to_lovelace((mem + steps).ceil())
}

/// Tiered reference-script fee.
///
/// Accumulates `sum(chunk_i * rate_i)` across every full
/// tier and the trailing partial tier, then applies `floor` exactly once at
/// the end. `rate_i` is multiplied by `multiplier` between tiers.
///
/// Reference: <https://github.com/IntersectMBO/cardano-ledger/blob/0cfbf861cfb456660a7b73281c6fb714a53d40f9/eras/conway/impl/src/Cardano/Ledger/Conway/Tx.hs#L111>
pub fn reference_scripts_fee(
    size: u64,
    stride: u64,
    base_rate: &RationalNumber,
    multiplier: &RationalNumber,
) -> Lovelace {
    let multiplier = into_safe_ratio(multiplier);

    assert!(stride > 0 && multiplier > SafeRatio::zero(), "stride and multiplier must be greater than 0");

    let stride = BigUint::from(stride);

    let mut rate = into_safe_ratio(base_rate);
    let mut fee = SafeRatio::zero();
    let mut size = BigUint::from(size);
    while size >= stride {
        fee += &rate * &stride;
        rate *= &multiplier;
        size -= &stride;
    }

    fee += rate * size;

    floor_to_lovelace(fee)
}

#[cfg(test)]
mod tests {
    use test_case::test_case;

    #[test_case(1_000, 25_600, 15, 1, 12, 10 => 15_000; "tier: within first tier (1000 bytes * 15)")]
    #[test_case(250, 100, 10, 1, 2, 1 => 5_000; "tier: spans 3 tiers (100*10 + 100*20 + 50*40)")]
    #[test_case(5, 5, 1, 3, 1, 1 => 1; "tier: final floor (floor(5/3) = 1)")]
    #[test_case(10, 5, 1, 3, 1, 1 => 3; "tier: rational accumulates across tiers (floor(5/3 + 5/3) = 3, not 2+2)")]
    #[test_case(0, 25_600, 15, 1, 15, 10 => 0; "haskell golden size=0")]
    #[test_case(25_600, 25_600, 15, 1, 15, 10 => 384_000; "haskell golden size=25_600")]
    #[test_case(51_200, 25_600, 15, 1, 15, 10 => 960_000; "haskell golden size=51_200")]
    #[test_case(76_800, 25_600, 15, 1, 15, 10 => 1_824_000; "haskell golden size=76_800")]
    #[test_case(102_400, 25_600, 15, 1, 15, 10 => 3_120_000; "haskell golden size=102_400")]
    #[test_case(128_000, 25_600, 15, 1, 15, 10 => 5_064_000; "haskell golden size=128_000")]
    #[test_case(153_600, 25_600, 15, 1, 15, 10 => 7_980_000; "haskell golden size=153_600")]
    #[test_case(179_200, 25_600, 15, 1, 15, 10 => 12_354_000; "haskell golden size=179_200")]
    #[test_case(204_800, 25_600, 15, 1, 15, 10 => 18_915_000; "haskell golden size=204_800")]
    fn tier_ref_script_fee(size: u64, stride: u64, base_n: u64, base_d: u64, mult_n: u64, mult_d: u64) -> u64 {
        super::reference_scripts_fee(
            size,
            stride,
            &crate::RationalNumber::new(base_n, base_d).expect("a valid rational number"),
            &crate::RationalNumber::new(mult_n, mult_d).expect("a valid rational number"),
        )
    }

    #[test_case(0, 0, 5, 1, 7, 1 => 0; "plutus: zero units")]
    #[test_case(1_000, 0, 5, 1, 7, 1 => 5_000; "plutus: mem axis only (1000 * 5/1)")]
    #[test_case(0, 1_000, 5, 1, 7, 1 => 7_000; "plutus: steps axis only (1000 * 7/1)")]
    #[test_case(1, 1, 1, 3, 1, 3 => 1; "plutus: ceiling on fractional sum (1/3 + 1/3 = 2/3, ceil = 1)")]
    #[test_case(10, 10, 1, 3, 1, 4 => 6; "plutus: different denominators (10/3 + 10/4 = 70/12, ceil = 6)")]
    fn plutus_exec_fee(mem: u64, steps: u64, mem_n: u64, mem_d: u64, step_n: u64, step_d: u64) -> u64 {
        super::plutus_execution_fee(
            crate::ExUnits { mem, steps },
            &crate::ExUnitPrices {
                mem_price: crate::RationalNumber::new(mem_n, mem_d).expect("a valid rational number"),
                step_price: crate::RationalNumber::new(step_n, step_d).expect("a valid rational number"),
            },
        )
    }
}
