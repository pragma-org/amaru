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
    cmp::Ordering,
    fmt,
    ops::{Add, Div, Mul, Neg, Sub},
    str::FromStr,
    sync::LazyLock,
};

use dashu_base::{Abs, DivRem, Sign};
use dashu_int::{IBig, UBig};

const WIDTH: usize = 34;

static PRECISION: LazyLock<IBig> = LazyLock::new(|| IBig::from(10).pow(WIDTH));

static EPS: LazyLock<IBig> = LazyLock::new(|| IBig::from(10).pow(WIDTH - 24));

static ONE: LazyLock<IBig> = LazyLock::new(|| PRECISION.clone());

static FIXED_DECIMAL_ONE: LazyLock<FixedDecimal> = LazyLock::new(|| FixedDecimal { value: ONE.clone() });

static E: LazyLock<IBig> = LazyLock::new(|| {
    let mut e = IBig::ZERO;
    ref_exp(&mut e, &ONE);
    e
});

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpOrdering {
    GT,
    LT,
    UNKNOWN,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExpCmpOrdering {
    pub iterations: u64,
    pub estimation: ExpOrdering,
    pub approx: FixedDecimal,
}

#[derive(Debug, Clone, PartialEq, PartialOrd)]
pub struct FixedDecimal {
    value: IBig,
}

impl FixedDecimal {
    pub const ZERO: Self = Self { value: IBig::ZERO };

    pub fn one() -> &'static Self {
        &FIXED_DECIMAL_ONE
    }

    pub fn new(n: IBig) -> Self {
        Self { value: n * &*PRECISION }
    }

    pub fn ln(&self) -> Self {
        let mut ln_x = Self::ZERO;
        if ref_ln(&mut ln_x.value, &self.value) { ln_x } else { unreachable!("ln of a value in (-inf,0] is undefined") }
    }

    pub fn exp_cmp(&self, max_n: u64, bound_self: i64, compare: &Self) -> ExpCmpOrdering {
        ref_exp_cmp(max_n, &self.value, bound_self, &compare.value)
    }
}

impl FromStr for FixedDecimal {
    type Err = dashu_base::ParseError;

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
        FixedDecimal { value: &self.value + &rhs.value }
    }
}

impl Neg for &FixedDecimal {
    type Output = FixedDecimal;

    fn neg(self) -> Self::Output {
        FixedDecimal { value: IBig::ZERO - &self.value }
    }
}

impl Sub for &FixedDecimal {
    type Output = FixedDecimal;

    fn sub(self, rhs: Self) -> Self::Output {
        FixedDecimal { value: &self.value - &rhs.value }
    }
}

impl Mul for &FixedDecimal {
    type Output = FixedDecimal;

    fn mul(self, rhs: Self) -> Self::Output {
        let mut value = &self.value * &rhs.value;
        scale(&mut value);
        FixedDecimal { value }
    }
}

impl Div for &FixedDecimal {
    type Output = FixedDecimal;

    fn div(self, rhs: Self) -> Self::Output {
        let mut result = FixedDecimal::ZERO;
        div(&mut result.value, &self.value, &rhs.value);
        result
    }
}

impl fmt::Display for FixedDecimal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fn print_fixedp(n: &IBig, precision: &IBig, width: usize) -> String {
            let (mut temp_q, mut temp_r) = n.div_rem(precision);

            let is_negative_q = temp_q < IBig::ZERO;
            let is_negative_r = temp_r < IBig::ZERO;

            if is_negative_q {
                temp_q = temp_q.abs();
            }
            if is_negative_r {
                temp_r = temp_r.abs();
            }

            let mut s = String::new();
            if is_negative_q || is_negative_r {
                s.push('-');
            }
            s.push_str(&temp_q.to_string());
            s.push('.');
            let r = temp_r.to_string();
            let r_len = r.len();
            // fill with zeroes up to width for the fractional part
            if r_len < width {
                s.push_str(&"0".repeat(width - r_len));
            }
            s.push_str(&r);
            s
        }

        write!(f, "{}", print_fixedp(&self.value, &PRECISION, WIDTH))
    }
}

/// Division with quotent and remainder
#[inline]
fn div_qr(q: &mut IBig, r: &mut IBig, x: &IBig, y: &IBig) {
    (*q, *r) = x.div_rem(y);
}

fn div(rop: &mut IBig, x: &IBig, y: &IBig) {
    let mut temp_q = IBig::ZERO;
    let mut temp_r = IBig::ZERO;
    let mut temp: IBig;
    div_qr(&mut temp_q, &mut temp_r, x, y);

    temp = &temp_q * &*PRECISION;
    temp_r = &temp_r * &*PRECISION;
    let temp_r2 = temp_r.clone();
    div_qr(&mut temp_q, &mut temp_r, &temp_r2, y);

    temp += &temp_q;
    *rop = temp;
}

fn div_round_ceil(x: &IBig, y: &IBig) -> IBig {
    let (q, r) = x.div_rem(y);
    if q.sign() == Sign::Positive && r != IBig::ZERO { q + IBig::ONE } else { q }
}

/// Entry point for 'exp' approximation. First does the scaling of 'x' to [0,1]
/// and then calls the continued fraction approximation function.
#[expect(clippy::expect_used)]
fn ref_exp(rop: &mut IBig, x: &IBig) -> i32 {
    let mut iterations = 0;
    match x.cmp(&IBig::ZERO) {
        Ordering::Equal => {
            // rop = 1
            rop.clone_from(&ONE);
        }
        Ordering::Less => {
            let x_ = -x;
            let mut temp = IBig::ZERO;
            iterations = ref_exp(&mut temp, &x_);
            // rop = 1 / temp
            div(rop, &ONE, &temp);
        }
        Ordering::Greater => {
            let n_exponent = div_round_ceil(x, &PRECISION);
            let x_ = x / &n_exponent;
            iterations = mp_exp_taylor(rop, 1000, &x_, &EPS);

            // rop = rop.pow(n)
            let n_exponent_i64: i64 = i64::try_from(&n_exponent).expect("n_exponent to_i64 failed");
            ipow(rop, &rop.clone(), n_exponent_i64);
        }
    }

    iterations
}

/// Entry point for 'ln' approximation. First does the necessary scaling, and
/// then calls the continued fraction calculation. For any value outside the
/// domain, i.e., 'x in (-inf,0]', the function returns '-INFINITY'.
fn ref_ln(rop: &mut IBig, x: &IBig) -> bool {
    let mut factor = IBig::ZERO;
    let mut x_ = IBig::ZERO;
    if x <= &IBig::ZERO {
        return false;
    }

    let n = find_e(x);

    *rop = IBig::from(n);
    *rop = &*rop * &*PRECISION;
    ref_exp(&mut factor, rop);

    div(&mut x_, x, &factor);

    x_ = &x_ - &*ONE;

    let x_2 = x_.clone();
    mp_ln_n(&mut x_, 1000, &x_2, &EPS);
    *rop = &*rop + &x_;

    true
}

fn find_e(x: &IBig) -> i64 {
    let mut x_: IBig = IBig::ZERO;
    let mut x__: IBig = E.clone();

    div(&mut x_, &ONE, &E);

    let mut l = -1;
    let mut u = 1;
    while &x_ > x || &x__ < x {
        x_ = &x_ * &x_;
        scale(&mut x_);

        x__ = &x__ * &x__;
        scale(&mut x__);

        l *= 2;
        u *= 2;
    }

    while l + 1 != u {
        let mid = l + ((u - l) / 2);

        ipow(&mut x_, &E, mid);
        if x < &x_ {
            u = mid;
        } else {
            l = mid;
        }
    }
    l
}

/// Taylor / MacLaurin series approximation
fn mp_exp_taylor(rop: &mut IBig, max_n: i32, x: &IBig, epsilon: &IBig) -> i32 {
    let mut divisor = ONE.clone();
    let mut last_x = ONE.clone();
    rop.clone_from(&ONE);
    let mut n = 0;
    while n < max_n {
        let mut next_x = x * &last_x;
        scale(&mut next_x);
        let next_x2 = next_x.clone();
        div(&mut next_x, &next_x2, &divisor);

        if (&next_x).abs() < epsilon.abs() {
            break;
        }

        divisor += &*ONE;
        *rop = &*rop + &next_x;
        last_x.clone_from(&next_x);
        n += 1;
    }

    n
}

fn scale(rop: &mut IBig) {
    let mut temp = IBig::ZERO;
    let mut a = IBig::ZERO;
    div_qr(&mut a, &mut temp, rop, &PRECISION);
    if *rop < IBig::ZERO && temp != IBig::ZERO {
        a -= IBig::ONE;
    }
    *rop = a;
}

/// Integer power
fn ipow(rop: &mut IBig, x: &IBig, n: i64) {
    if n < 0 {
        let mut temp = IBig::ZERO;
        ipow_(&mut temp, x, -n);
        div(rop, &ONE, &temp);
    } else {
        ipow_(rop, x, n);
    }
}

/// Integer power internal function
fn ipow_(rop: &mut IBig, x: &IBig, n: i64) {
    if n == 0 {
        rop.clone_from(&ONE);
    } else if n % 2 == 0 {
        let mut res = IBig::ZERO;
        ipow_(&mut res, x, n / 2);
        *rop = &res * &res;
        scale(rop);
    } else {
        let mut res = IBig::ZERO;
        ipow_(&mut res, x, n - 1);
        *rop = res * x;
        scale(rop);
    }
}

/// Compute an approximation of 'ln(1 + x)' via continued fractions. Either for a
///    maximum of 'maxN' iterations or until the absolute difference between two
///    succeeding convergents is smaller than 'eps'. Assumes 'x' to be within
///    [1,e).
fn mp_ln_n(rop: &mut IBig, max_n: i32, x: &IBig, epsilon: &IBig) {
    let mut ba: IBig;
    let mut aa: IBig;
    let mut ab: IBig;
    let mut bb: IBig;
    let mut a_: IBig;
    let mut b_: IBig;
    let mut diff: IBig;
    let mut convergent: IBig = IBig::ZERO;
    let mut last: IBig = IBig::ZERO;
    let mut first = true;
    let mut n = 1;

    let mut a: IBig;
    let mut b = ONE.clone();

    let mut an_m2 = ONE.clone();
    let mut bn_m2 = IBig::ZERO;
    let mut an_m1 = IBig::ZERO;
    let mut bn_m1 = ONE.clone();

    let mut curr_a = 1;

    while n <= max_n + 2 {
        let curr_a_2 = curr_a * curr_a;
        a = x * IBig::from(curr_a_2);
        if n > 1 && n % 2 == 1 {
            curr_a += 1;
        }

        ba = &b * &an_m1;
        scale(&mut ba);
        aa = &a * &an_m2;
        scale(&mut aa);
        a_ = &ba + &aa;

        bb = &b * &bn_m1;
        scale(&mut bb);
        ab = &a * &bn_m2;
        scale(&mut ab);
        b_ = &bb + &ab;

        div(&mut convergent, &a_, &b_);

        if first {
            first = false;
        } else {
            diff = &convergent - &last;
            if diff.abs() < epsilon.abs() {
                break;
            }
        }

        last.clone_from(&convergent);

        n += 1;
        an_m2.clone_from(&an_m1);
        bn_m2.clone_from(&bn_m1);
        an_m1.clone_from(&a_);
        bn_m1.clone_from(&b_);

        b += &*ONE;
    }

    *rop = convergent;
}

/// `bound_x` is the bound for exp in the interval x is chosen from
/// `compare` the value to compare to
///
/// if the result is GT, then the computed value is guaranteed to be greater, if
/// the result is LT, the computed value is guaranteed to be less than
/// `compare`. In the case of `UNKNOWN` no conclusion was possible for the
/// selected precision.
///
/// Lagrange remainder require knowledge of the maximum value to compute the
/// maximal error of the remainder.
fn ref_exp_cmp(max_n: u64, x: &IBig, bound_x: i64, compare: &IBig) -> ExpCmpOrdering {
    let mut n = 0u64;
    let mut divisor: IBig;
    let mut next_x: IBig;
    let mut error: IBig;
    let mut upper: IBig;
    let mut lower: IBig;
    let mut error_term: IBig;

    divisor = ONE.clone();
    error = x.clone();

    let mut approx = FixedDecimal::one().clone();
    let mut estimate = ExpOrdering::UNKNOWN;
    let rop = &mut approx.value;
    while n < max_n {
        next_x = error.clone();
        if (&next_x).abs() < (&*EPS).abs() {
            break;
        }
        divisor += &*ONE;

        // update error estimation, this is initially bound_x * x and in general
        // bound_x * x^(n+1)/(n + 1)!  we use `error` to store the x^n part and a
        // single integral multiplication with the bound
        error *= x;
        scale(&mut error);
        let e2 = error.clone();
        div(&mut error, &e2, &divisor);
        error_term = &error * IBig::from(bound_x);
        *rop = &*rop + &next_x;

        /* compare is guaranteed to be above overall result */
        upper = &*rop + &error_term;
        if compare > &upper {
            estimate = ExpOrdering::GT;
            n += 1;
            break;
        }

        /* compare is guaranteed to be below overall result */
        lower = &*rop - &error_term;
        if compare < &lower {
            estimate = ExpOrdering::LT;
            n += 1;
            break;
        }
        n += 1;
    }

    ExpCmpOrdering { iterations: n, estimation: estimate, approx }
}
