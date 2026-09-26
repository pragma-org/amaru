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

use std::fmt;

use crate::{RationalNumber, cbor};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, cbor::Encode)]
#[serde(try_from = "RationalNumber")]
#[cbor(transparent)]
#[repr(transparent)]
pub struct UnitRationalNumber(#[n(0)] RationalNumber);

impl fmt::Display for UnitRationalNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl<'b, C> cbor::decode::Decode<'b, C> for UnitRationalNumber {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        let rational: RationalNumber = d.decode_with(ctx)?;
        rational.try_into().map_err(|e| minicbor::decode::Error::message(&e))
    }
}

impl UnitRationalNumber {
    pub fn new(numerator: u64, denominator: u64) -> Result<Self, String> {
        let rational = RationalNumber::new(numerator, denominator)?;
        if numerator > denominator {
            return Err("the rational value must belong to the interval [0, 1]".to_string());
        }
        Ok(UnitRationalNumber(rational))
    }

    /// The underlying ratio, for callers that only need to read the numerator and denominator.
    pub fn as_ratio(&self) -> &RationalNumber {
        &self.0
    }

    pub fn numerator(&self) -> u64 {
        self.0.numerator()
    }

    pub fn denominator(&self) -> u64 {
        self.0.denominator()
    }
}

impl From<UnitRationalNumber> for RationalNumber {
    fn from(unit_rational: UnitRationalNumber) -> Self {
        unit_rational.0
    }
}

impl TryFrom<RationalNumber> for UnitRationalNumber {
    type Error = String;

    fn try_from(rational: RationalNumber) -> Result<Self, Self::Error> {
        Self::new(rational.numerator(), rational.denominator())
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
        pub fn any_unit_rational_number()(
            denominator in 1..u64::MAX,
        )(
          delta in 0..denominator,
          denominator in Just(denominator),
      ) -> UnitRationalNumber {
            UnitRationalNumber(RationalNumber::new(denominator - delta, denominator).unwrap())
        }
    }
}
