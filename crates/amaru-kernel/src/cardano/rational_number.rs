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

use crate::cbor;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct RationalNumber {
    pub numerator: u64,
    pub denominator: u64,
}

impl fmt::Display for RationalNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.numerator, self.denominator)
    }
}

impl<'b, C> cbor::decode::Decode<'b, C> for RationalNumber {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        // TODO: Enforce tag == 30
        d.tag()?;
        cbor::heterogeneous_array(d, |d, assert_len| {
            assert_len(2)?;
            Ok(RationalNumber { numerator: d.decode_with(ctx)?, denominator: d.decode_with(ctx)? })
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

#[cfg(any(test, feature = "test-utils"))]
pub use tests::*;

#[cfg(any(test, feature = "test-utils"))]
mod tests {
    use proptest::prelude::*;

    use super::*;

    prop_compose! {
        pub fn any_rational_number()(
            numerator in any::<u64>(),
            denominator in 1..u64::MAX,
        ) -> RationalNumber {
            RationalNumber {
                numerator,
                denominator,
            }
        }
    }
}
