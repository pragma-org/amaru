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

use std::{cmp::Ordering, fmt};

use num::{BigUint, rational::Ratio};

use crate::{Lovelace, UnitRationalNumber, cbor};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct RationalNumber {
    numerator: u64,
    denominator: u64,
}

impl PartialOrd for RationalNumber {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for RationalNumber {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.numerator as u128 * other.denominator as u128).cmp(&(other.numerator as u128 * self.denominator as u128))
    }
}

impl fmt::Display for RationalNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.numerator, self.denominator)
    }
}

impl<'b, C> cbor::decode::Decode<'b, C> for RationalNumber {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        cbor::expect_tag(d, cbor::Tag::new(30))?;
        cbor::heterogeneous_array(d, |d, assert_len| {
            assert_len(2)?;
            let numerator = d.decode_with(ctx)?;
            let denominator = d.decode_with(ctx)?;
            RationalNumber::new(numerator, denominator).map_err(|e| minicbor::decode::Error::message(&e))
        })
    }
}

impl<C> cbor::encode::Encode<C> for RationalNumber {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.tag(cbor::Tag::new(30))?;
        e.array(2)?;
        e.encode_with(self.numerator, ctx)?;
        e.encode_with(self.denominator, ctx)?;
        Ok(())
    }
}

impl<'de> serde::Deserialize<'de> for RationalNumber {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(serde::Deserialize)]
        struct Repr {
            numerator: u64,
            denominator: u64,
        }

        let Repr { numerator, denominator } = Repr::deserialize(d)?;
        RationalNumber::new(numerator, denominator).map_err(serde::de::Error::custom)
    }
}

impl RationalNumber {
    /// Build a rational in lowest terms.
    /// This makes sure that PartialEq / Eq are correct and makes the comparison
    /// with encoded values easier in the conformance test suite.
    pub fn new(numerator: u64, denominator: u64) -> Result<Self, String> {
        if denominator == 0 {
            return Err("rational denominator cannot be zero".to_string());
        }
        let divisor = gcd(numerator, denominator);
        Ok(Self { numerator: numerator / divisor, denominator: denominator / divisor })
    }

    pub fn numerator(&self) -> u64 {
        self.numerator
    }

    pub fn denominator(&self) -> u64 {
        self.denominator
    }
}

impl From<UnitRationalNumber> for SafeRatio {
    fn from(r: UnitRationalNumber) -> Self {
        into_safe_ratio(&r.into())
    }
}

/// Binary GCD, iterative so a pathological pair cannot blow the stack.
fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

// ------------------------------------------------------------------- SafeRatio

pub type SafeRatio = Ratio<BigUint>;

pub fn safe_ratio(numerator: u64, denominator: u64) -> SafeRatio {
    SafeRatio::new(BigUint::from(numerator), BigUint::from(denominator))
}

pub fn into_safe_ratio(ratio: &RationalNumber) -> SafeRatio {
    SafeRatio::new(BigUint::from(ratio.numerator()), BigUint::from(ratio.denominator()))
}

pub fn floor_to_lovelace(n: SafeRatio) -> Lovelace {
    Lovelace::try_from(n.floor().to_integer())
        .unwrap_or_else(|_| unreachable!("always fits in a u64; otherwise we've exceeded the max Ada supply."))
}

impl From<RationalNumber> for SafeRatio {
    fn from(r: RationalNumber) -> Self {
        into_safe_ratio(&r)
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub use tests::*;

#[cfg(any(test, feature = "test-utils"))]
mod tests {
    use proptest::prelude::*;

    use super::*;

    prop_compose! {
        #[expect(clippy::unwrap_used)]
        pub fn any_rational_number()(
            numerator in any::<u64>(),
            denominator in 1..u64::MAX,
        ) -> RationalNumber {
            RationalNumber::new(numerator, denominator).unwrap()
        }
    }
}
