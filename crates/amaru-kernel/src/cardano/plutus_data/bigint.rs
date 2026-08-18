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

use std::cmp::Ordering;

use crate::{Int, cbor, plutus_data::BoundedBytes};

#[derive(Debug, Clone, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum BigInt {
    Int(Int),
    BigUInt(BoundedBytes),
    BigNInt(BoundedBytes),
}

impl PartialOrd for BigInt {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for BigInt {
    fn cmp(&self, other: &Self) -> Ordering {
        fn to_bytes(i: &BigInt) -> (bool, Vec<u8>) {
            match i {
                BigInt::Int(i) => {
                    let i = Into::<i128>::into(*i);
                    (i < 0, i.abs().to_be_bytes().into_iter().skip_while(|b| b == &0).collect())
                }
                BigInt::BigUInt(bs) => (false, bs.iter().skip_while(|b| b == &&0).copied().collect()),
                BigInt::BigNInt(bs) => (true, bs.iter().skip_while(|b| b == &&0).copied().collect()),
            }
        }

        let (left_is_negative, left) = to_bytes(self);

        let (right_is_negative, right) = to_bytes(other);

        if left.is_empty() && right.is_empty() {
            return Ordering::Equal;
        }

        if left_is_negative && !right_is_negative {
            return Ordering::Less;
        }

        if !left_is_negative && right_is_negative {
            return Ordering::Greater;
        }

        let when_positives = match left.len().cmp(&right.len()) {
            Ordering::Equal => left.cmp(&right),
            ordering @ Ordering::Less | ordering @ Ordering::Greater => ordering,
        };

        if left_is_negative && right_is_negative { when_positives.reverse() } else { when_positives }
    }
}

impl<'b, C: cbor::HasProtocolVersion> cbor::Decode<'b, C> for BigInt {
    #[expect(clippy::wildcard_enum_match_arm)]
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        let datatype = d.datatype()?;

        match datatype {
            cbor::data::Type::U8
            | cbor::data::Type::U16
            | cbor::data::Type::U32
            | cbor::data::Type::U64
            | cbor::data::Type::I8
            | cbor::data::Type::I16
            | cbor::data::Type::I32
            | cbor::data::Type::I64
            | cbor::data::Type::Int => Ok(Self::Int(d.decode_with(ctx)?)),
            cbor::data::Type::Tag => {
                let tag = d.tag()?;
                if tag == cbor::IanaTag::PosBignum.tag() {
                    Ok(Self::BigUInt(d.decode_with(ctx)?))
                } else if tag == cbor::IanaTag::NegBignum.tag() {
                    Ok(Self::BigNInt(d.decode_with(ctx)?))
                } else {
                    Err(cbor::decode::Error::message("invalid cbor tag for big int"))
                }
            }
            _ => Err(cbor::decode::Error::message("invalid cbor data type for big int")),
        }
    }
}

impl<C: cbor::HasProtocolVersion> cbor::Encode<C> for BigInt {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        match self {
            BigInt::Int(x) => {
                e.encode_with(x, ctx)?;
            }
            BigInt::BigUInt(x) => {
                e.tag(cbor::IanaTag::PosBignum)?;
                e.encode_with(x, ctx)?;
            }
            BigInt::BigNInt(x) => {
                e.tag(cbor::IanaTag::NegBignum)?;
                e.encode_with(x, ctx)?;
            }
        };

        Ok(())
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub use tests::*;

#[cfg(any(test, feature = "test-utils"))]
mod tests {
    use proptest::prelude::*;

    use super::BigInt;
    use crate::plutus_data::any_bounded_bytes;

    pub fn any_bigint() -> impl Strategy<Value = BigInt> {
        prop_oneof![
            any::<i64>().prop_map(|i| BigInt::Int(i.into())),
            any_bounded_bytes().prop_map(BigInt::BigUInt),
            any_bounded_bytes().prop_map(BigInt::BigNInt),
        ]
    }
}
