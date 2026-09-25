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

use stacksafe::{StackSafe, stacksafe};

use crate::{
    Bytes, Hash, Hasher, NonEmptyVec, cbor, from_cbor, size::DATUM, to_cbor, utils::string::blanket_try_from_hex_bytes,
};

mod bigint;
pub use bigint::*;

mod constr;
pub use constr::*;

// ---------------------------------------------------------------------------------------------
// MemoizedPlutusData
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct MemoizedPlutusData {
    original_bytes: Vec<u8>,
    // NOTE: This field isn't meant to be public, nor should we create any direct mutable
    // references to it. Reason being that this object is mostly meant to be read-only, and any
    // change to the 'data' should be reflected onto the 'original_bytes'.
    data: PlutusData,
}

impl Eq for MemoizedPlutusData {}
impl PartialEq for MemoizedPlutusData {
    fn eq(&self, rhs: &Self) -> bool {
        self.data.eq(&rhs.data)
    }
}

impl serde::Serialize for MemoizedPlutusData {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        crate::utils::serde::bytes::serialize(&self.original_bytes, serializer)
    }
}

impl<'de> serde::Deserialize<'de> for MemoizedPlutusData {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let original_bytes = crate::utils::serde::bytes::deserialize(deserializer)?;
        let data = from_cbor(&original_bytes).ok_or_else(|| serde::de::Error::custom("failed to decode PlutusData"))?;
        Ok(Self { original_bytes, data })
    }
}

impl schemars::JsonSchema for MemoizedPlutusData {
    fn schema_name() -> String {
        "PlutusData".to_string()
    }

    fn json_schema(_gen: &mut schemars::r#gen::SchemaGenerator) -> schemars::schema::Schema {
        crate::utils::serde::bytes::json_schema("hex-encoded Plutus data")
    }

    fn is_referenceable() -> bool {
        false
    }
}

impl MemoizedPlutusData {
    pub fn new(data: PlutusData) -> Self {
        Self { original_bytes: to_cbor(&data), data }
    }

    pub fn data(&self) -> &PlutusData {
        &self.data
    }

    pub fn original_bytes(&self) -> &[u8] {
        &self.original_bytes
    }

    pub fn hash(&self) -> Hash<32> {
        Hasher::<256>::hash(&self.original_bytes)
    }
}

impl From<MemoizedPlutusData> for String {
    fn from(plutus_data: MemoizedPlutusData) -> Self {
        hex::encode(&plutus_data.original_bytes[..])
    }
}

impl TryFrom<&str> for MemoizedPlutusData {
    type Error = String;
    fn try_from(s: &str) -> Result<Self, Self::Error> {
        blanket_try_from_hex_bytes(s, |original_bytes, data| Self { original_bytes, data })
    }
}

impl TryFrom<String> for MemoizedPlutusData {
    type Error = String;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::try_from(s.as_str())
    }
}

impl<'b, C: cbor::HasProtocolVersion> cbor::Decode<'b, C> for MemoizedPlutusData {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        let (data, original_bytes) = cbor::tee(d, |d| d.decode_with(ctx))?;
        Ok(Self { original_bytes: original_bytes.to_vec(), data })
    }
}

impl<C> cbor::Encode<C> for MemoizedPlutusData {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        _ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.writer_mut().write_all(&self.original_bytes[..]).map_err(cbor::encode::Error::write)
    }
}

// ---------------------------------------------------------------------------------------------
// PlutusData
// ---------------------------------------------------------------------------------------------

/// A safe API for PlutusData which controls access to inner elements to ensure they remain
/// stack-safe.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct PlutusData(StackSafe<PlutusDataTree>);

impl PlutusData {
    fn new(data: PlutusDataTree) -> Self {
        Self(StackSafe::new(data))
    }

    #[cfg(test)]
    #[stacksafe]
    fn into_inner(self) -> PlutusDataTree {
        self.0.into_inner()
    }

    pub fn constr(constr: Constr<PlutusData>) -> Self {
        Self::new(PlutusDataTree::Constr(constr))
    }

    #[stacksafe]
    pub fn as_constr(&self) -> Option<&Constr<Self>> {
        if let PlutusDataTree::Constr(constr) = self.0.deref() { Some(constr) } else { None }
    }

    pub fn map(elems: Vec<(Self, Self)>) -> Self {
        Self::new(PlutusDataTree::Map(elems.into_iter().collect()))
    }

    #[stacksafe]
    pub fn as_map(&self) -> Option<&[(Self, Self)]> {
        if let PlutusDataTree::Map(elems) = self.0.deref() { Some(elems) } else { None }
    }

    pub fn array(elems: Vec<Self>) -> Self {
        Self::new(PlutusDataTree::Array(elems.into_iter().collect()))
    }

    #[stacksafe]
    pub fn as_array(&self) -> Option<&[Self]> {
        if let PlutusDataTree::Array(elems) = self.0.deref() { Some(elems) } else { None }
    }

    pub fn int(i: BigInt) -> Self {
        Self::new(PlutusDataTree::BigInt(i))
    }

    #[stacksafe]
    pub fn as_int(&self) -> Option<&BigInt> {
        if let PlutusDataTree::BigInt(i) = self.0.deref() { Some(i) } else { None }
    }

    pub fn bytes(bytes: Vec<u8>) -> Self {
        Self::new(PlutusDataTree::BoundedBytes(bytes.into()))
    }

    #[stacksafe]
    pub fn as_bytes(&self) -> Option<&[u8]> {
        if let PlutusDataTree::BoundedBytes(bytes) = self.0.deref() { Some(bytes) } else { None }
    }
}

impl<C: cbor::HasProtocolVersion> cbor::encode::Encode<C> for PlutusData {
    #[stacksafe]
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.encode_with(self.0.deref(), ctx)?;
        Ok(())
    }
}

impl<'b, C: cbor::HasProtocolVersion> cbor::decode::Decode<'b, C> for PlutusData {
    #[stacksafe]
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        d.decode_with(ctx).map(Self::new)
    }
}

// ---------------------------------------------------------------------------------------------
// PlutusDataTree
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
enum PlutusDataTree {
    Constr(Constr<PlutusData>),
    Map(Vec<(PlutusData, PlutusData)>),
    Array(Vec<PlutusData>),
    BigInt(BigInt),
    BoundedBytes(Bytes),
}

// NOTE: Dubious choices of encoding in this encoder?
//
// This PlutusDataTree encoder follows the same rules and quirks as the Haskell node, which can be
// summarized as:
//
// 1. Non-empty arrays encoded using indefinite length. When empty, they're encoded using definite length.
// 2. Maps are always encoded with definite length, even when empty.
// 3. Constr fields follow the same rules as arrays.
// 4. Bytes are encoded as definite length if less than 64 bytes, and with indefinite in chunks of
//    up-to 64 bytes when larger.
impl<C: cbor::HasProtocolVersion> cbor::encode::Encode<C> for PlutusDataTree {
    #[stacksafe]
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
                for (k, v) in kvs.iter() {
                    e.encode_with(k, ctx)?;
                    e.encode_with(v, ctx)?;
                }
            }
            Self::Array(array) => {
                if array.is_empty() {
                    e.array(0)?;
                } else {
                    e.begin_array()?;
                    for elem in array.iter() {
                        e.encode_with(elem, ctx)?;
                    }
                    e.end()?;
                }
            }
            Self::BigInt(i) => {
                e.encode_with(i, ctx)?;
            }
            Self::BoundedBytes(bytes) => {
                cbor::encode_bytestring(e, bytes)?;
            }
        };

        Ok(())
    }
}

impl<'b, C: cbor::HasProtocolVersion> cbor::decode::Decode<'b, C> for PlutusDataTree {
    #[expect(clippy::wildcard_enum_match_arm)]
    #[stacksafe]
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

            cbor::data::Type::Bytes | cbor::data::Type::BytesIndef => Ok(Self::BoundedBytes(decode_bounded_bytes(d)?)),

            any => Err(cbor::decode::Error::message(format!("bad cbor data type ({any:?}) for plutus data"))),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// BoundedBytes
// ---------------------------------------------------------------------------------------------

pub use amaru_minicbor_extra::MAX_BOUNDED_BYTES_CHUNK;

/// Decode a Plutus data byte string, accepting both the definite-length form and the
/// indefinite-length (chunked) form as long as no piece exceeds [`MAX_BOUNDED_BYTES_CHUNK`].
pub fn decode_bounded_bytes(d: &mut cbor::Decoder<'_>) -> Result<Bytes, cbor::decode::Error> {
    let mut bytes = Vec::new();
    for chunk in d.bytes_iter()? {
        let chunk = chunk?;
        amaru_minicbor_extra::assert_bounded_chunk(chunk)?;
        bytes.extend_from_slice(chunk);
    }
    Ok(Bytes::from(bytes))
}

// ---------------------------------------------------------------------------------------------
// PlutusDataSet
// ---------------------------------------------------------------------------------------------

// FIXME: Bytes duplication
//
// This type duplicates bytes from all `PlutusData`; which is not necessary. To reconstruct
// the original bytes, we only need to store the original envelope, and defer to the `inner` plutus
// data for each item original bytes.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct PlutusDataSet {
    // FIXME: wrong serde 'skip' & default.
    // This looks wrong; we cannot skip bytes here. Should be fixed once the note above is adressed.
    #[serde(skip, default = "crate::Bytes::default")]
    original_bytes: Bytes,
    inner: NonEmptyVec<MemoizedPlutusData>,
}

impl<C: cbor::HasProtocolVersion> cbor::Encode<C> for PlutusDataSet {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        self.inner.encode(e, ctx)
    }
}

impl<'b, C: cbor::HasProtocolVersion> cbor::Decode<'b, C> for PlutusDataSet {
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
        Self(plutus_data.iter().map(|data| (data.hash(), data.data())).collect())
    }
}

impl<'a> From<&'a PlutusDataSet> for PlutusDatums<'a> {
    fn from(plutus_data: &'a PlutusDataSet) -> Self {
        Self::from(&**plutus_data)
    }
}

// ---------------------------------------------------------------------------------------------
// VariableEncodingPlutusData
// ---------------------------------------------------------------------------------------------

#[cfg(any(test, feature = "test-utils"))]
mod variable_encoding_plutus_data {
    use proptest::prelude::*;

    use super::PlutusData;
    use crate::{
        Bytes, MemoizedPlutusData, cbor,
        plutus_data::{BigInt, Constr, VariableEncodingConstr, any_bigint, any_bounded_bytes},
        utils::cbor::{CborArray, CborMap},
    };

    #[derive(Debug, Clone)]
    pub enum VariableEncodingPlutusData {
        Constr(VariableEncodingConstr<VariableEncodingPlutusData>),
        Map(CborMap<VariableEncodingPlutusData, VariableEncodingPlutusData>),
        BigInt(BigInt),
        BoundedBytes(Bytes),
        Array(CborArray<VariableEncodingPlutusData>),
    }

    impl TryFrom<VariableEncodingPlutusData> for MemoizedPlutusData {
        type Error = ();
        fn try_from(data: VariableEncodingPlutusData) -> Result<Self, Self::Error> {
            PlutusData::try_from(data).map(MemoizedPlutusData::new)
        }
    }

    impl TryFrom<VariableEncodingPlutusData> for PlutusData {
        type Error = ();
        fn try_from(data: VariableEncodingPlutusData) -> Result<Self, Self::Error> {
            Ok(match data {
                VariableEncodingPlutusData::BigInt(i) => Self::int(i),
                VariableEncodingPlutusData::BoundedBytes(i) => Self::bytes(i.to_vec()),
                VariableEncodingPlutusData::Array(xs) => Self::array(match xs {
                    CborArray::Def(xs) | CborArray::Indef(xs) => {
                        xs.into_iter().map(|x| x.try_into()).collect::<Result<Vec<_>, _>>()?
                    }
                }),
                VariableEncodingPlutusData::Map(xs) => Self::map(match xs {
                    CborMap::Def(xs) | CborMap::Indef(xs) => xs
                        .into_iter()
                        .map(|(k, v)| k.try_into().and_then(|k| v.try_into().map(|v| (k, v))))
                        .collect::<Result<Vec<_>, _>>()?,
                }),
                VariableEncodingPlutusData::Constr(VariableEncodingConstr { tag, any_constructor, fields }) => {
                    Self::constr(Constr {
                        tag,
                        any_constructor,
                        fields: match fields {
                            CborArray::Def(xs) | CborArray::Indef(xs) => {
                                xs.into_iter().map(|x| x.try_into()).collect::<Result<_, _>>()?
                            }
                        },
                    })
                }
            })
        }
    }

    impl<C: cbor::HasProtocolVersion> cbor::encode::Encode<C> for VariableEncodingPlutusData {
        fn encode<W: cbor::encode::Write>(
            &self,
            e: &mut cbor::Encoder<W>,
            ctx: &mut C,
        ) -> Result<(), cbor::encode::Error<W::Error>> {
            match self {
                Self::Constr(a) => {
                    e.encode_with(a, ctx)?;
                }
                Self::Map(a) => {
                    e.encode_with(a, ctx)?;
                }
                Self::BigInt(a) => {
                    e.encode_with(a, ctx)?;
                }
                Self::BoundedBytes(a) => {
                    cbor::encode_bytestring(e, a)?;
                }
                Self::Array(a) => {
                    e.encode_with(a, ctx)?;
                }
            };

            Ok(())
        }
    }

    impl VariableEncodingPlutusData {
        pub fn any(depth: u8) -> impl Strategy<Value = Self> {
            let int = any_bigint().prop_map(Self::BigInt);

            let bytes = any_bounded_bytes().prop_map(Self::BoundedBytes);

            if depth > 0 {
                let constr = VariableEncodingConstr::any(depth).prop_map(Self::Constr);

                let array = (any::<bool>(), prop::collection::vec(Self::any(depth - 1), 0..depth as usize)).prop_map(
                    |(is_def, xs)| Self::Array(if is_def { CborArray::Def(xs) } else { CborArray::Indef(xs) }),
                );

                let map = (
                    any::<bool>(),
                    prop::collection::vec((Self::any(depth - 1), Self::any(depth - 1)), 0..depth as usize),
                )
                    .prop_map(|(is_def, kvs)| Self::Map(if is_def { CborMap::Def(kvs) } else { CborMap::Indef(kvs) }));

                prop_oneof![int, bytes, constr, array, map].boxed()
            } else {
                prop_oneof![int, bytes].boxed()
            }
        }
    }

    proptest! {
        #[test]
        fn roundtrip_hex_encoded_str(original_data in VariableEncodingPlutusData::any(3)) {
            let original_bytes = crate::to_cbor(&original_data);
            let result = MemoizedPlutusData::try_from(hex::encode(&original_bytes)).unwrap();

            assert_eq!(Some(&result), MemoizedPlutusData::try_from(original_data).ok().as_ref());
            assert_eq!(result.original_bytes(), &original_bytes);
        }
    }

    proptest! {
        #[test]
        fn roundtrip_cbor(original_data in VariableEncodingPlutusData::any(3)) {
            let original_bytes = crate::to_cbor(&original_data);
            let result: MemoizedPlutusData = crate::from_cbor(&original_bytes).unwrap();

            assert_eq!(Some(&result), MemoizedPlutusData::try_from(original_data).ok().as_ref());
            assert_eq!(result.original_bytes(), &original_bytes);
        }
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
    use crate::plutus_data::{any_bigint, any_constr};

    pub fn any_bounded_bytes() -> impl Strategy<Value = Bytes> {
        any::<Vec<u8>>().prop_map(Bytes::from)
    }

    pub fn any_memoized_plutus_data(depth: u8) -> impl Strategy<Value = MemoizedPlutusData> {
        any_plutus_data(depth).prop_map(MemoizedPlutusData::new)
    }

    pub fn any_plutus_data(depth: u8) -> impl Strategy<Value = PlutusData> {
        let int = any_bigint().prop_map(PlutusData::int);

        let bytes = any_bounded_bytes().prop_map(Vec::from).prop_map(PlutusData::bytes);

        if depth > 0 {
            let constr = any_constr(depth).prop_map(PlutusData::constr);

            let array =
                prop::collection::vec(any_plutus_data(depth - 1), 0..depth as usize).prop_map(PlutusData::array);

            let map =
                prop::collection::vec((any_plutus_data(depth - 1), any_plutus_data(depth - 1)), 0..depth as usize)
                    .prop_map(PlutusData::map);

            prop_oneof![int, bytes, constr, array, map].boxed()
        } else {
            prop_oneof![int, bytes].boxed()
        }
    }

    pub fn int(i: i64) -> PlutusData {
        PlutusData::int(BigInt::Int(i.into()))
    }

    pub fn biguint(bs: &[u8]) -> PlutusData {
        PlutusData::int(BigInt::BigUInt(Bytes::from(bs.to_vec())))
    }

    pub fn bignint(bs: &[u8]) -> PlutusData {
        PlutusData::int(BigInt::BigNInt(Bytes::from(bs.to_vec())))
    }

    pub fn bytes(bs: &[u8]) -> PlutusData {
        PlutusData::bytes(bs.to_vec())
    }

    pub fn array(xs: &[PlutusData]) -> PlutusData {
        PlutusData::array(xs.to_vec())
    }

    pub fn map(kvs: &[(PlutusData, PlutusData)]) -> PlutusData {
        PlutusData::map(kvs.to_vec())
    }

    pub fn constr(tag: u64, fields: &[PlutusData]) -> PlutusData {
        PlutusData::constr(Constr { tag, any_constructor: None, fields: fields.to_vec() })
    }

    pub fn constr_any(any_constructor: u64, fields: &[PlutusData]) -> PlutusData {
        PlutusData::constr(Constr { tag: 102, any_constructor: Some(any_constructor), fields: fields.to_vec() })
    }

    #[cfg(test)]
    mod stack_overflow {
        use crate::{MemoizedPlutusData, PlutusData, from_cbor, to_cbor, utils::stack};

        const TRANSACTION_MAX_SIZE: usize = 16384;

        #[test]
        fn deeply_nested_array() {
            let max_depth = TRANSACTION_MAX_SIZE / 2;
            let (lhs, rhs) = rayon::join(
                || nest_with(max_depth, leaf(0), |data| super::array(&[data])),
                || nest_with(max_depth, leaf(1), |data| super::array(&[data])),
            );

            stack::with_stack_size(stack::STACK_SIZE_512KIB, move || {
                assert!(lhs != rhs);
                assert!(lhs == lhs.clone());
                let bytes = to_cbor(&lhs);
                assert!(from_cbor(&bytes) == Some(lhs));
            })
            .expect("couldn't run or spawn thread")
        }

        #[test]
        fn deeply_nested_map() {
            let max_depth = TRANSACTION_MAX_SIZE / 3;
            let (lhs, rhs) = rayon::join(
                || nest_with(max_depth, leaf(0), |data| super::map(&[(super::bytes(&[]), data)])),
                || nest_with(max_depth, leaf(1), |data| super::map(&[(super::bytes(&[]), data)])),
            );

            stack::with_stack_size(stack::STACK_SIZE_512KIB, move || {
                assert!(lhs != rhs);
                assert!(lhs == lhs.clone());
                let bytes = to_cbor(&lhs);
                assert!(from_cbor(&bytes) == Some(lhs));
            })
            .expect("couldn't run or spawn thread")
        }

        fn leaf(byte: u8) -> PlutusData {
            super::bytes(&[byte; 1])
        }

        pub fn nest_with(
            mut depth: usize,
            mut leaf: PlutusData,
            nest: impl Fn(PlutusData) -> PlutusData,
        ) -> MemoizedPlutusData {
            while depth > 0 {
                leaf = nest(leaf);
                depth -= 1;
            }
            MemoizedPlutusData::new(leaf)
        }
    }

    #[cfg(test)]
    mod internal {
        use std::cmp::Ordering;

        use proptest::prelude::*;
        use test_case::test_case;

        use super::{
            super::{PlutusData, PlutusDataTree},
            any_memoized_plutus_data, array, bignint, biguint, bytes, constr, constr_any, int, map,
        };
        use crate::{MemoizedPlutusData, cbor, plutus_data::BigInt};

        proptest! {
            #[test]
            fn cbor_roundtrip(original_data in any_memoized_plutus_data(3)) {
                let bytes = cbor::to_vec(&original_data).unwrap();
                let data: MemoizedPlutusData = cbor::decode(&bytes).unwrap();
                assert_eq!(data, original_data);
            }
        }

        #[test]
        fn invalid_string() {
            assert!(MemoizedPlutusData::try_from("foo".to_string()).is_err());
        }

        #[test]
        fn json_is_hex_string_and_cbor_is_byte_string() {
            let data = MemoizedPlutusData::new(PlutusData::bytes(vec![1, 2, 3]));
            let payload = data.original_bytes().to_vec();
            crate::utils::serde::bytes::assert_json_hex_and_cbor_bstr(&data, &payload);
        }

        fn definite(len: u8) -> Vec<u8> {
            [vec![0x58, len], vec![0; len as usize]].concat()
        }

        fn chunked(lens: &[u8]) -> Vec<u8> {
            [vec![0x5f], lens.iter().flat_map(|len| definite(*len)).collect(), vec![0xff]].concat()
        }

        #[test_case(definite(64) => matches Ok(PlutusDataTree::BoundedBytes(_)))]
        #[test_case(definite(65) => matches Err(_))]
        #[test_case(chunked(&[64, 64]) => matches Ok(PlutusDataTree::BoundedBytes(_)))]
        #[test_case(chunked(&[65]) => matches Err(_))]
        #[test_case([vec![0xc2], definite(64)].concat() => matches Ok(PlutusDataTree::BigInt(BigInt::BigUInt(_))))]
        #[test_case([vec![0xc2], definite(65)].concat() => matches Err(_))]
        #[test_case([vec![0xc3], chunked(&[64, 1])].concat() => matches Ok(PlutusDataTree::BigInt(BigInt::BigNInt(_))))]
        #[test_case([vec![0xc3], chunked(&[1, 65])].concat() => matches Err(_))]
        fn decode_bounded_bytes_limit(bytes: Vec<u8>) -> Result<PlutusDataTree, cbor::decode::Error> {
            cbor::from_cbor_no_leftovers(&bytes).map(|plutus_data: MemoizedPlutusData| plutus_data.data.into_inner())
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
