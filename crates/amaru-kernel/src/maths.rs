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

use std::{
    ops::{Add, AddAssign, Div, DivAssign, Mul, MulAssign, Neg, Sub, SubAssign},
    str::FromStr,
    sync::LazyLock,
};

use dashu_base::{DivRem, ParseError, Sign};
use dashu_int::{IBig, UBig};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpOrdering {
    /// The comparison value is strictly below the exponential lower bound.
    LT,
    /// The comparison value is at or above the exponential upper bound.
    GT,
    /// The supplied iteration budget was exhausted without a decision.
    UNKNOWN,
}

#[derive(Debug, Clone, PartialEq, PartialOrd)]
pub struct FixedDecimal(IBig);

const WIDTH: usize = 34;

static PRECISION: LazyLock<IBig> = LazyLock::new(|| IBig::from(10).pow(WIDTH));

static FIXED_DECIMAL_ONE: LazyLock<FixedDecimal> = LazyLock::new(|| FixedDecimal(PRECISION.clone()));

#[expect(clippy::expect_used)]
static E: LazyLock<FixedDecimal> =
    LazyLock::new(|| FixedDecimal(IBig::from_str("27182818284590452353602874043083282").expect("infallible")));

impl FixedDecimal {
    pub const ZERO: Self = Self(IBig::ZERO);

    pub fn one() -> &'static Self {
        &FIXED_DECIMAL_ONE
    }

    pub fn new(n: IBig) -> Self {
        Self(n * &*PRECISION)
    }

    pub fn sign(&self) -> Sign {
        self.0.sign()
    }
}

impl serde::Serialize for FixedDecimal {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'a> serde::Deserialize<'a> for FixedDecimal {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'a>,
    {
        let s = String::deserialize(deserializer)?;
        IBig::from_str(s.as_str()).map_err(serde::de::Error::custom).map(Self)
    }
}

impl FromStr for FixedDecimal {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::new(IBig::from_str(s)?))
    }
}

impl From<UBig> for FixedDecimal {
    fn from(n: UBig) -> Self {
        Self::new(IBig::from(n))
    }
}

impl From<u64> for FixedDecimal {
    fn from(n: u64) -> Self {
        Self::new(IBig::from(n))
    }
}

impl From<&[u8]> for FixedDecimal {
    fn from(n: &[u8]) -> Self {
        Self::from(UBig::from_be_bytes(n))
    }
}

impl Add for &FixedDecimal {
    type Output = FixedDecimal;

    fn add(self, rhs: Self) -> Self::Output {
        FixedDecimal(&self.0 + &rhs.0)
    }
}

impl Add for FixedDecimal {
    type Output = FixedDecimal;

    fn add(self, rhs: Self) -> Self::Output {
        Self(self.0 + rhs.0)
    }
}

impl AddAssign for FixedDecimal {
    fn add_assign(&mut self, rhs: Self) {
        let lhs = std::mem::replace(&mut self.0, IBig::ZERO);
        self.0 = lhs + rhs.0;
    }
}

impl AddAssign<&FixedDecimal> for FixedDecimal {
    fn add_assign(&mut self, rhs: &FixedDecimal) {
        let lhs = std::mem::replace(&mut self.0, IBig::ZERO);
        self.0 = lhs + &rhs.0;
    }
}

impl Neg for &FixedDecimal {
    type Output = FixedDecimal;

    fn neg(self) -> Self::Output {
        FixedDecimal(IBig::ZERO - &self.0)
    }
}

impl Neg for FixedDecimal {
    type Output = FixedDecimal;

    fn neg(self) -> Self::Output {
        Self(-self.0)
    }
}

impl Sub for &FixedDecimal {
    type Output = FixedDecimal;

    fn sub(self, rhs: Self) -> Self::Output {
        FixedDecimal(&self.0 - &rhs.0)
    }
}

impl Sub for FixedDecimal {
    type Output = FixedDecimal;

    fn sub(self, rhs: Self) -> Self::Output {
        Self(self.0 - rhs.0)
    }
}

impl SubAssign for FixedDecimal {
    fn sub_assign(&mut self, rhs: Self) {
        let lhs = std::mem::replace(&mut self.0, IBig::ZERO);
        self.0 = lhs - rhs.0;
    }
}

impl SubAssign<&FixedDecimal> for FixedDecimal {
    fn sub_assign(&mut self, rhs: &FixedDecimal) {
        let lhs = std::mem::replace(&mut self.0, IBig::ZERO);
        self.0 = lhs - &rhs.0;
    }
}

impl Mul for &FixedDecimal {
    type Output = FixedDecimal;

    fn mul(self, rhs: Self) -> Self::Output {
        let mut value = &self.0 * &rhs.0;
        scale(&mut value);
        FixedDecimal(value)
    }
}

impl Mul for FixedDecimal {
    type Output = FixedDecimal;

    fn mul(self, rhs: Self) -> Self::Output {
        Self(scale_owned(self.0 * rhs.0))
    }
}

impl MulAssign for FixedDecimal {
    fn mul_assign(&mut self, rhs: Self) {
        let lhs = std::mem::replace(&mut self.0, IBig::ZERO);
        self.0 = scale_owned(lhs * rhs.0);
    }
}

impl MulAssign<&FixedDecimal> for FixedDecimal {
    fn mul_assign(&mut self, rhs: &FixedDecimal) {
        let lhs = std::mem::replace(&mut self.0, IBig::ZERO);
        self.0 = scale_owned(lhs * &rhs.0);
    }
}

impl Div for &FixedDecimal {
    type Output = FixedDecimal;

    fn div(self, rhs: Self) -> Self::Output {
        let numerator = &self.0 * &*PRECISION;

        let (mut quotient, remainder) = numerator.div_rem(&rhs.0);
        if remainder != IBig::ZERO && self.sign() != rhs.sign() {
            quotient -= IBig::ONE;
        }

        FixedDecimal(quotient)
    }
}

impl Div for FixedDecimal {
    type Output = FixedDecimal;

    fn div(self, rhs: Self) -> Self::Output {
        let lhs_sign = self.sign();
        let rhs_sign = rhs.sign();
        let numerator = self.0 * &*PRECISION;

        let (mut quotient, remainder) = numerator.div_rem(rhs.0);
        if remainder != IBig::ZERO && lhs_sign != rhs_sign {
            quotient -= IBig::ONE;
        }

        Self(quotient)
    }
}

impl DivAssign for FixedDecimal {
    fn div_assign(&mut self, rhs: Self) {
        let lhs = std::mem::replace(&mut self.0, IBig::ZERO);
        self.0 = (Self(lhs) / rhs).0;
    }
}

impl DivAssign<&FixedDecimal> for FixedDecimal {
    fn div_assign(&mut self, rhs: &FixedDecimal) {
        let lhs = std::mem::replace(&mut self.0, IBig::ZERO);
        self.0 = (&Self(lhs) / rhs).0;
    }
}

impl FixedDecimal {
    pub fn exp_cmp(&self, rhs: &Self) -> ExpOrdering {
        let err_bound = FixedDecimal::new(IBig::from(3));
        let max_iterations = 1000_usize;

        let mut n = 0;
        let mut err = self.clone();
        let mut acc = FixedDecimal::one().clone();
        let mut divisor = FixedDecimal::one().clone();

        while n < max_iterations {
            acc = &acc + &err;
            divisor = &divisor + FixedDecimal::one();
            err = &(&err * self) / &divisor;

            let mut error_term: FixedDecimal = &err * &err_bound;
            if error_term <= FixedDecimal::ZERO {
                error_term = error_term.neg();
            }

            if rhs >= &(&acc + &error_term) {
                return ExpOrdering::GT;
            } else if rhs < &(&acc - &error_term) {
                return ExpOrdering::LT;
            } else {
                n += 1;
            }
        }

        ExpOrdering::UNKNOWN
    }

    pub fn ln(&self) -> Self {
        assert!(self > &Self::ZERO);
        let (n, x) = self.split_ln();
        Self::new(IBig::from(n)) + x.ln_cf(1000)
    }

    // Approximate ln(1+x) for x in [0, +∞).
    //
    // a_1 = x, a_{2k} = a_{2k+1} = x·k^2, k >= 1
    // b_n = n, n >= 0
    fn ln_cf(&self, max_n: usize) -> Self {
        assert!(self >= &Self::ZERO);

        let epsilon = Self::new(IBig::ONE) / Self::new(IBig::from(10).pow(24));
        let mut a_nm2 = Self::one().clone();
        let mut b_nm2 = Self::ZERO;
        let mut a_nm1 = Self::ZERO;
        let mut b_nm1 = Self::one().clone();
        let mut result = Self::ZERO;

        // The first convergent is x itself, so even max_n == 0 performs one step,
        // matching the reference implementation's continued-fraction recurrence.
        for n in 0..=max_n {
            let a_n = if n == 0 {
                self.clone()
            } else {
                let k = n.div_ceil(2) as u64;
                self.clone() * Self::from(k * k)
            };
            let b_n = Self::from((n + 1) as u64);

            let numerator = (&b_n * &a_nm1) + (&a_n * &a_nm2);
            let denominator = (&b_n * &b_nm1) + (&a_n * &b_nm2);
            let convergent = &numerator / &denominator;

            if n > 0 {
                let mut difference = &convergent - &result;
                if difference < Self::ZERO {
                    difference = -difference;
                }
                if difference < epsilon {
                    return convergent;
                }
            }

            result = convergent;
            a_nm2 = a_nm1;
            a_nm1 = numerator;
            b_nm2 = b_nm1;
            b_nm1 = denominator;
        }

        result
    }

    fn split_ln(&self) -> (isize, Self) {
        // find `n` such that `e^n <= x < e^(n+1)`
        let (lower, upper) = self.bound();
        let n = self.bisect(lower, upper);
        let y = E.ipow(n);
        (n, (self / &y) - FixedDecimal::one().clone())
    }

    // Simple way to find integer powers that bound self. At every step the bounds are
    // doubled. With self > 0, the calculated bound is such that `e^l <= x <= e^u`,
    fn bound(&self) -> (isize, isize) {
        let mut lo = FixedDecimal::one() / &*E;
        let mut hi = E.clone();

        let mut l: isize = -1;
        let mut u: isize = 1;

        while self < &lo || self > &hi {
            lo = &lo * &lo;
            hi = &hi * &hi;
            l *= 2;
            u *= 2;
        }

        (l, u)
    }

    // Bisect bounds to find the smallest integer power such that factor^n <= x < factor^(n+1)
    fn bisect(&self, mut l: isize, mut u: isize) -> isize {
        while 1 + l != u {
            let mid = l + ((u - l) / 2);
            if self < &E.ipow(mid) {
                u = mid;
            } else {
                l = mid;
            }
        }
        l
    }

    fn ipow(&self, n: isize) -> Self {
        enum Operation {
            MultiplyByBase,
            Square,
        }

        fn ipow_inner(x: &FixedDecimal, n: usize) -> FixedDecimal {
            let mut operations = Vec::new();
            let mut n = n;

            while n > 0 {
                if n.is_multiple_of(2) {
                    operations.push(Operation::Square);
                    n /= 2;
                } else {
                    operations.push(Operation::MultiplyByBase);
                    n -= 1;
                }
            }

            let mut result = FixedDecimal::one().clone();

            while let Some(operation) = operations.pop() {
                match operation {
                    Operation::MultiplyByBase => result *= x,
                    Operation::Square => result = &result * &result,
                }
            }

            result
        }

        let result = ipow_inner(self, n.unsigned_abs());
        if n < 0 { Self::one() / &result } else { result }
    }
}

// --------------------------------------------------------------------------------------------------
// Helpers
// --------------------------------------------------------------------------------------------------

fn scale(rop: &mut IBig) {
    *rop = scale_owned(std::mem::replace(rop, IBig::ZERO));
}

fn scale_owned(value: IBig) -> IBig {
    let is_negative = value < IBig::ZERO;
    let (mut quotient, remainder) = value.div_rem(&*PRECISION);
    if is_negative && remainder != IBig::ZERO {
        quotient -= IBig::ONE;
    }
    quotient
}

// --------------------------------------------------------------------------------------------------
// Tests
// --------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn raw(s: &str) -> FixedDecimal {
        FixedDecimal(IBig::from_str(s).unwrap())
    }

    #[test]
    fn node_primitives_match() {
        let rows = include_str!("../tests/data/maths/node-primitives.tsv");
        assert_eq!(rows.lines().count(), 32);
        for (index, row) in rows.lines().enumerate() {
            let columns: Vec<_> = row.split('\t').collect();
            let [operation, a, b, expected] = columns.as_slice() else {
                panic!("malformed primitive row {}: {row}", index + 1);
            };
            let actual = match *operation {
                "multiply" => raw(a) * raw(b),
                "divide" => raw(a) / raw(b),
                "ratio" => raw(a) / raw(b),
                _ => panic!("unknown operation in row {}: {row}", index + 1),
            };
            assert_eq!(actual, raw(expected), "primitive row {}: {row}", index + 1);
        }
    }

    #[test]
    fn node_comparisons_match() {
        let rows = include_str!("../tests/data/maths/node-comparisons.tsv");
        assert_eq!(rows.lines().count(), 9);
        for (index, row) in rows.lines().enumerate() {
            let columns: Vec<_> = row.split('\t').collect();
            let [compare, x, ordering, _iterations] = columns.as_slice() else {
                panic!("malformed comparison row {}: {row}", index + 1);
            };
            let expected = match *ordering {
                "ABOVE" => ExpOrdering::GT,
                "BELOW" => ExpOrdering::LT,
                "MAX_REACHED" => ExpOrdering::UNKNOWN,
                _ => panic!("unknown ordering in row {}: {row}", index + 1),
            };
            let actual = raw(x).exp_cmp(&raw(compare));
            assert_eq!(
                actual,
                expected,
                "comparison row {} (negative remainders require an absolute bound): {row}",
                index + 1
            );
        }
    }

    #[test]
    fn owned_and_assignment_arithmetic_match_borrowed() {
        let lhs = FixedDecimal::from(7u64);
        let rhs = FixedDecimal::from(2u64);

        assert_eq!(lhs.clone() + rhs.clone(), &lhs + &rhs);
        assert_eq!(lhs.clone() - rhs.clone(), &lhs - &rhs);
        assert_eq!(lhs.clone() * rhs.clone(), &lhs * &rhs);
        assert_eq!(lhs.clone() / rhs.clone(), &lhs / &rhs);
        assert_eq!(-lhs.clone(), -&lhs);

        let mut actual = lhs.clone();
        actual += rhs.clone();
        assert_eq!(actual, &lhs + &rhs);

        let mut actual = lhs.clone();
        actual += &rhs;
        assert_eq!(actual, &lhs + &rhs);

        let mut actual = lhs.clone();
        actual -= rhs.clone();
        assert_eq!(actual, &lhs - &rhs);

        let mut actual = lhs.clone();
        actual -= &rhs;
        assert_eq!(actual, &lhs - &rhs);

        let mut actual = lhs.clone();
        actual *= rhs.clone();
        assert_eq!(actual, &lhs * &rhs);

        let mut actual = lhs.clone();
        actual *= &rhs;
        assert_eq!(actual, &lhs * &rhs);

        let mut actual = lhs.clone();
        actual /= rhs.clone();
        assert_eq!(actual, &lhs / &rhs);

        let mut actual = lhs;
        actual /= &rhs;
        assert_eq!(actual, &FixedDecimal::from(7u64) / &rhs);
    }

    #[test]
    fn exp_cmp_resolves_below_epsilon() {
        let x = FixedDecimal(IBig::from(10).pow(22));
        let q = FixedDecimal(&*PRECISION + IBig::from(10).pow(22) + IBig::from(10).pow(9));
        assert_eq!(x.exp_cmp(&q), ExpOrdering::LT);
    }

    #[test]
    fn exp_cmp_upper_equality_is_above() {
        let q = &FixedDecimal::from(7) / &FixedDecimal::from(2);
        assert_eq!(FixedDecimal::one().exp_cmp(&q), ExpOrdering::GT)
    }

    #[test]
    fn exp_cmp_lower_equality_is_below() {
        let q = FixedDecimal::one() / &FixedDecimal::from(2);
        assert_eq!(FixedDecimal::one().exp_cmp(&q), ExpOrdering::LT);
    }

    #[test]
    fn exp_cmp_exhausts_budget() {
        let q = FixedDecimal::one();
        assert_eq!(FixedDecimal::from(1_000_000).exp_cmp(q), ExpOrdering::UNKNOWN);
    }

    #[test]
    #[should_panic]
    fn division_by_zero_panics() {
        let _ = FixedDecimal::one() / &FixedDecimal::ZERO;
    }

    #[test]
    fn active_slot_log_matches_node() {
        let coeff = FixedDecimal::one() / &FixedDecimal::from(20);
        assert_eq!((FixedDecimal::one() - &coeff).ln(), raw("-512932943875505334261962382072846"));
    }

    #[test]
    fn continued_fraction_respects_maximum_convergent() {
        let x = FixedDecimal::from(1u64);

        assert_eq!(x.ln_cf(0), x);
        assert_eq!(x.ln_cf(1), &FixedDecimal::from(2u64) / &FixedDecimal::from(3u64));
    }

    #[test]
    fn integer_power_preserves_haskell_rounding_order() {
        let e = E.clone();
        let e2 = &e * &e;
        let e3 = &e * &e2;
        let expected = &e3 * &e3;

        assert_eq!(e.ipow(6), expected);
    }

    proptest! {
        #[test]
        fn fixed_decimal_serialisation_roundtrip(negative in any::<bool>(), magnitude in any::<u128>()) {
            let magnitude = IBig::from(magnitude);
            let value = FixedDecimal::new(if negative { -magnitude } else { magnitude });
            let serialized = serde_json::to_string(&value).unwrap();
            prop_assert_eq!(serde_json::from_str(&serialized).map_err(|e| e.to_string()), Ok(value));
        }

        #[test]
        fn fixed_decimal_from_str_interpret_as_ibig(negative in any::<bool>(), magnitude in any::<u128>()) {
            let magnitude = if negative { -IBig::from(magnitude) } else { IBig::from(magnitude) };
            prop_assert_eq!(FixedDecimal::from_str(&magnitude.to_string()), Ok(FixedDecimal::new(magnitude)));
        }
    }
}
