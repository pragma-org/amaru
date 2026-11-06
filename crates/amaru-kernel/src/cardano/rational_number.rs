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

use dashu_base::{Gcd, Sign, UnsignedAbs};
use dashu_int::IBig;
use num::{BigUint, rational::Ratio};
#[cfg(any(test, feature = "test-utils"))]
use proptest::prelude::{Arbitrary, BoxedStrategy, Strategy, any};

use crate::{Lovelace, UnitRationalNumber, cbor};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct RationalNumber {
    numerator: u64,
    denominator: u64,
}

impl RationalNumber {
    pub fn zero() -> Self {
        RationalNumber { numerator: 0, denominator: 1 }
    }

    pub fn one() -> Self {
        RationalNumber { numerator: 1, denominator: 1 }
    }
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
    /// Read the numerator and denominator as unbounded integers, reduce the ratio, then require that it fits in
    /// `u64`. The Haskell node does the same: `-1 / -2` and `2^65 / 2^66` are valid encodings of `1 / 2`.
    fn decode(d: &mut cbor::Decoder<'b>, _ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        cbor::expect_tag(d, cbor::Tag::new(30))?;
        cbor::heterogeneous_array(d, |d, assert_len| {
            assert_len(2)?;
            let numerator = cbor::decode_integer(d)?;
            let denominator = cbor::decode_integer(d)?;
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
    /// Build a rational in lowest terms from integers of any size.
    /// This makes sure that PartialEq / Eq are correct and makes the comparison
    /// with encoded values easier in the conformance test suite.
    ///
    /// Signs are normalized, so `-1 / -2` is `1 / 2`. Fails when the denominator is zero, or when the reduced
    /// ratio is negative or does not fit in `u64`.
    pub fn new(numerator: impl Into<IBig>, denominator: impl Into<IBig>) -> Result<Self, String> {
        let (numerator, denominator) = (numerator.into(), denominator.into());
        if denominator == IBig::ZERO {
            return Err("rational denominator cannot be zero".to_string());
        }
        let negative = (numerator.sign() == Sign::Negative) != (denominator.sign() == Sign::Negative);
        let numerator = numerator.unsigned_abs();
        let denominator = denominator.unsigned_abs();
        let divisor = (&numerator).gcd(&denominator);
        match (u64::try_from(&numerator / &divisor), u64::try_from(&denominator / &divisor)) {
            (Ok(numerator), Ok(denominator)) if !negative || numerator == 0 => Ok(Self { numerator, denominator }),
            _ => Err("rational value must be non-negative and fit in 64 bits once reduced".to_string()),
        }
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

#[cfg(any(test, feature = "test-utils"))]
impl Arbitrary for RationalNumber {
    type Parameters = ();
    type Strategy = BoxedStrategy<Self>;

    #[expect(clippy::unwrap_used)]
    fn arbitrary_with(_: Self::Parameters) -> Self::Strategy {
        (any::<u64>(), 1..u64::MAX)
            .prop_map(|(numerator, denominator)| RationalNumber::new(numerator, denominator).unwrap())
            .boxed()
    }
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
