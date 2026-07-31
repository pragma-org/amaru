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

use std::{collections::BTreeMap, ops::Deref};

use crate::{Bytes, Hash, MemoizedPlutusData, NonEmptyVec, cbor, empty_bytes, size::DATUM};

mod bigint;
pub use bigint::*;

mod bounded_bytes;
pub use bounded_bytes::*;

mod constr;
pub use constr::*;

// ---------------------------------------------------------------------------------------------
// PlutusData
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub enum PlutusData {
    Constr(Constr<PlutusData>),
    Map(Vec<(PlutusData, PlutusData)>),
    Array(Vec<PlutusData>),
    BigInt(BigInt),
    BoundedBytes(BoundedBytes),
}

// NOTE: Dubious choices of encoding in this encoder?
//
// This PlutusData encoder follows the same rules and quirks as the Haskell node, which can be
// summarized as:
//
// 1. Non-empty arrays encoded using indefinite length. When empty, they're encoded using definite length.
// 2. Maps are always encoded with definite length, even when empty.
// 3. Constr fields follow the same rules as arrays.
// 4. Bytes are encoded as definite length if less than 64 bytes, and with indefinite in chunks of
//    up-to 64 bytes when larger.
impl<C> cbor::encode::Encode<C> for PlutusData {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        match self {
            Self::Constr(constr) => {
                e.encode_with(constr, ctx)?;
            }
            Self::Map(kvs) => {
                e.map(kvs.len() as u64)?;
                for (k, v) in kvs {
                    e.encode_with(k, ctx)?;
                    e.encode_with(v, ctx)?;
                }
            }
            Self::Array(array) => {
                if array.is_empty() {
                    e.array(0)?;
                } else {
                    e.begin_array()?;
                    for elem in array {
                        e.encode_with(elem, ctx)?;
                    }
                    e.end()?;
                }
            }
            Self::BigInt(i) => {
                e.encode_with(i, ctx)?;
            }
            Self::BoundedBytes(bytes) => {
                e.encode_with(bytes, ctx)?;
            }
        };

        Ok(())
    }
}

impl<'b, C> cbor::decode::Decode<'b, C> for PlutusData {
    #[expect(clippy::wildcard_enum_match_arm)]
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        match d.datatype()? {
            cbor::data::Type::Tag => {
                let mut probe = d.probe();
                let tag = probe.tag()?;

                if tag == cbor::IanaTag::PosBignum.tag() || tag == cbor::IanaTag::NegBignum.tag() {
                    Ok(Self::BigInt(d.decode_with(ctx)?))
                } else {
                    match tag.as_u64() {
                        (121..=127) | (1280..=1400) | 102 => Ok(Self::Constr(d.decode_with(ctx)?)),
                        _ => Err(cbor::decode::Error::message("unknown tag for plutus data tag")),
                    }
                }
            }

            cbor::data::Type::Map | cbor::data::Type::MapIndef => {
                Ok(Self::Map(d.map_iter_with(ctx)?.collect::<Result<_, _>>()?))
            }

            cbor::data::Type::Array | cbor::data::Type::ArrayIndef => Ok(Self::Array(d.decode_with(ctx)?)),

            cbor::data::Type::U8
            | cbor::data::Type::U16
            | cbor::data::Type::U32
            | cbor::data::Type::U64
            | cbor::data::Type::I8
            | cbor::data::Type::I16
            | cbor::data::Type::I32
            | cbor::data::Type::I64
            | cbor::data::Type::Int => Ok(Self::BigInt(d.decode_with(ctx)?)),

            cbor::data::Type::Bytes => Ok(Self::BoundedBytes(d.decode_with(ctx)?)),
            cbor::data::Type::BytesIndef => {
                let mut full = Vec::new();

                for slice in d.bytes_iter()? {
                    full.extend(slice?);
                }

                Ok(Self::BoundedBytes(BoundedBytes::from(full)))
            }

            any => Err(cbor::decode::Error::message(format!("bad cbor data type ({any:?}) for plutus data"))),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// PlutusDataSet
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct PlutusDataSet {
    #[serde(skip, default = "empty_bytes")]
    original_bytes: Bytes,
    inner: NonEmptyVec<MemoizedPlutusData>,
}

impl<C> cbor::Encode<C> for PlutusDataSet {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        self.inner.encode(e, ctx)
    }
}

impl<'b, C> cbor::Decode<'b, C> for PlutusDataSet {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        let (inner, bytes) = cbor::tee(d, |d| NonEmptyVec::<MemoizedPlutusData>::decode(d, ctx))?;
        Ok(Self { original_bytes: Bytes::from(bytes.to_vec()), inner })
    }
}

impl PlutusDataSet {
    pub fn original_bytes(&self) -> &[u8] {
        &self.original_bytes
    }
}

impl Deref for PlutusDataSet {
    type Target = NonEmptyVec<MemoizedPlutusData>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

// ---------------------------------------------------------------------------------------------
// PlutusDatums
// ---------------------------------------------------------------------------------------------

/// The datums supplied as witnesses in a transaction, keyed by hash.
///
/// A lookup table from a datum [`struct@Hash`] to the [`PlutusData`] it commits to. This is what
/// resolves a hash-only datum ([`MemoizedDatum::Hash`](crate::MemoizedDatum)) on a spent output back to the actual datum value;
/// inline datums carry their value already and need no entry here.
#[derive(Debug, Default)]
pub struct PlutusDatums<'a>(pub BTreeMap<Hash<DATUM>, &'a PlutusData>);

impl<'a> From<&'a NonEmptyVec<MemoizedPlutusData>> for PlutusDatums<'a> {
    fn from(plutus_data: &'a NonEmptyVec<MemoizedPlutusData>) -> Self {
        Self(plutus_data.iter().map(|data| (data.hash(), data.as_ref())).collect())
    }
}

impl<'a> From<&'a PlutusDataSet> for PlutusDatums<'a> {
    fn from(plutus_data: &'a PlutusDataSet) -> Self {
        Self::from(&**plutus_data)
    }
}

// ---------------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------------

#[cfg(any(test, feature = "test-utils"))]
pub use tests::*;

#[cfg(any(test, feature = "test-utils"))]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::plutus_data::{any_bigint, any_bounded_bytes, any_constr};

    pub fn any_plutus_data(depth: u8) -> BoxedStrategy<PlutusData> {
        let int = any_bigint().prop_map(PlutusData::BigInt);

        let bytes = any_bounded_bytes().prop_map(PlutusData::BoundedBytes);

        if depth > 0 {
            let constr = any_constr(depth).prop_map(PlutusData::Constr);

            let array =
                prop::collection::vec(any_plutus_data(depth - 1), 0..depth as usize).prop_map(PlutusData::Array);

            let map =
                prop::collection::vec((any_plutus_data(depth - 1), any_plutus_data(depth - 1)), 0..depth as usize)
                    .prop_map(PlutusData::Map);

            prop_oneof![int, bytes, constr, array, map].boxed()
        } else {
            prop_oneof![int, bytes].boxed()
        }
    }

    #[cfg(test)]
    mod internal {
        use std::cmp::Ordering;

        use proptest::prelude::*;
        use test_case::test_case;

        use super::any_plutus_data;
        use crate::{
            PlutusData, cbor,
            plutus_data::{BigInt, BoundedBytes, Constr},
        };

        proptest! {
            #[test]
            fn cbor_roundtrip(original_data in any_plutus_data(3)) {
                let bytes = cbor::to_vec(&original_data).unwrap();
                let data: PlutusData = cbor::decode(&bytes).unwrap();
                assert_eq!(data, original_data);
            }
        }

        fn int(i: i64) -> PlutusData {
            PlutusData::BigInt(BigInt::Int(i.into()))
        }

        fn biguint(bs: &[u8]) -> PlutusData {
            PlutusData::BigInt(BigInt::BigUInt(BoundedBytes::from(bs.to_vec())))
        }

        fn bignint(bs: &[u8]) -> PlutusData {
            PlutusData::BigInt(BigInt::BigNInt(BoundedBytes::from(bs.to_vec())))
        }

        fn bytes(bs: &[u8]) -> PlutusData {
            PlutusData::BoundedBytes(BoundedBytes::from(bs.to_vec()))
        }

        fn array(xs: &[PlutusData]) -> PlutusData {
            PlutusData::Array(xs.to_vec())
        }

        fn map(kvs: &[(PlutusData, PlutusData)]) -> PlutusData {
            PlutusData::Map(kvs.to_vec())
        }

        fn constr(tag: u64, fields: &[PlutusData]) -> PlutusData {
            PlutusData::Constr(Constr { tag, any_constructor: None, fields: fields.to_vec() })
        }

        fn constr_any(any_constructor: u64, fields: &[PlutusData]) -> PlutusData {
            PlutusData::Constr(Constr { tag: 102, any_constructor: Some(any_constructor), fields: fields.to_vec() })
        }

        // Bytes <-> ...
        #[test_case(bytes(&[]), bytes(&[]) => Ordering::Equal)]
        #[test_case(bytes(&[1, 2, 3]), bytes(&[4, 5, 6]) => Ordering::Less)]
        #[test_case(bytes(&[1, 2, 3]), bytes(&[1, 2, 3]) => Ordering::Equal)]
        #[test_case(bytes(&[4, 5, 6]), bytes(&[1, 2, 3]) => Ordering::Greater)]
        #[test_case(bytes(&[1, 2, 3]), bytes(&[2, 2, 3]) => Ordering::Less)]
        #[test_case(bytes(&[1, 2, 3]), bytes(&[1, 2]) => Ordering::Greater)]
        #[test_case(bytes(&[2, 2]), bytes(&[1, 2, 3]) => Ordering::Greater)]
        #[test_case(bytes(&[]), constr(121, &[]) => Ordering::Greater)]
        #[test_case(bytes(&[]), map(&[]) => Ordering::Greater)]
        #[test_case(bytes(&[]), array(&[]) => Ordering::Greater)]
        #[test_case(bytes(&[]), int(0) => Ordering::Greater)]
        // Int <-> ...
        #[test_case(int(42), int(14) => Ordering::Greater)]
        #[test_case(int(14), int(14) => Ordering::Equal)]
        #[test_case(int(14), int(42) => Ordering::Less)]
        #[test_case(int(0), int(-1) => Ordering::Greater)]
        #[test_case(int(-2), int(-1) => Ordering::Less)]
        #[test_case(int(0), biguint(&[0]) => Ordering::Equal)]
        #[test_case(int(14), biguint(&[14]) => Ordering::Equal)]
        #[test_case(int(14), biguint(&[42]) => Ordering::Less)]
        #[test_case(biguint(&[14]), int(42) => Ordering::Less)]
        #[test_case(biguint(&[42]), int(14) => Ordering::Greater)]
        #[test_case(biguint(&[14, 255]), int(42) => Ordering::Greater)]
        #[test_case(bignint(&[0]), int(0) => Ordering::Equal)]
        #[test_case(bignint(&[14, 255]), int(-42) => Ordering::Less)]
        #[test_case(biguint(&[]), int(0) => Ordering::Equal)]
        #[test_case(biguint(&[0, 0, 1]), int(1) => Ordering::Equal)]
        #[test_case(int(0), constr(121, &[]) => Ordering::Greater)]
        #[test_case(int(0), map(&[]) => Ordering::Greater)]
        #[test_case(int(0), array(&[]) => Ordering::Greater)]
        #[test_case(int(0), bytes(&[]) => Ordering::Less)]
        // Array <-> ...
        #[test_case(array(&[]), array(&[]) => Ordering::Equal)]
        #[test_case(array(&[int(14), int(42)]), array(&[int(14), int(42)]) => Ordering::Equal)]
        #[test_case(array(&[int(14), int(42)]), array(&[int(15)]) => Ordering::Less)]
        #[test_case(array(&[int(14), int(42)]), array(&[int(1), int(2), int(3)]) => Ordering::Greater)]
        #[test_case(array(&[]), constr(121, &[]) => Ordering::Greater)]
        #[test_case(array(&[]), map(&[]) => Ordering::Greater)]
        #[test_case(array(&[]), int(0) => Ordering::Less)]
        #[test_case(array(&[]), bytes(&[]) => Ordering::Less)]
        // Map <--> ...
        #[test_case(map(&[]), map(&[]) => Ordering::Equal)]
        #[test_case(map(&[(int(14), int(42))]), map(&[(int(14), int(41))]) => Ordering::Greater)]
        #[test_case(map(&[(int(14), int(41))]), map(&[(int(14), int(42))]) => Ordering::Less)]
        #[test_case(map(&[(int(14), int(42))]), map(&[(int(14), int(42))]) => Ordering::Equal)]
        #[test_case(map(&[(int(14), int(42))]), map(&[(int(14), int(42)), (int(1), int(999))]) => Ordering::Less)]
        #[test_case(map(&[(int(15), int(42))]), map(&[(int(14), int(42)), (int(1), int(999))]) => Ordering::Greater)]
        #[test_case(map(&[]), constr(121, &[]) => Ordering::Greater)]
        #[test_case(map(&[]), array(&[]) => Ordering::Less)]
        #[test_case(map(&[]), int(0) => Ordering::Less)]
        #[test_case(map(&[]), bytes(&[]) => Ordering::Less)]
        // Constr <-->
        #[test_case(constr(121, &[]), constr(121, &[]) => Ordering::Equal)]
        #[test_case(constr(122, &[]), constr(121, &[]) => Ordering::Greater)]
        #[test_case(constr(122, &[]), constr(121, &[int(999)]) => Ordering::Greater)]
        #[test_case(constr(126, &[int(999)]), constr(1281, &[]) => Ordering::Less)]
        #[test_case(constr_any(0, &[]), constr(121, &[]) => Ordering::Equal)]
        #[test_case(constr_any(1, &[]), constr(121, &[]) => Ordering::Greater)]
        #[test_case(constr_any(7, &[int(14)]), constr(1280, &[]) => Ordering::Greater)]
        #[test_case(constr_any(7, &[int(14)]), constr(1281, &[]) => Ordering::Less)]
        #[test_case(constr_any(121, &[]), map(&[]) => Ordering::Less)]
        #[test_case(constr_any(121, &[]), array(&[]) => Ordering::Less)]
        #[test_case(constr_any(121, &[]), int(0) => Ordering::Less)]
        #[test_case(constr_any(121, &[]), bytes(&[]) => Ordering::Less)]
        fn ordering(left: PlutusData, right: PlutusData) -> Ordering {
            left.cmp(&right)
        }
    }
}
