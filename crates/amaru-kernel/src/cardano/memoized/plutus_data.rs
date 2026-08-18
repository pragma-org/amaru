// Copyright 2025 PRAGMA
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

use crate::{Hash, Hasher, PlutusData, cbor, utils::string::blanket_try_from_hex_bytes};

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(try_from = "&str")]
#[serde(into = "String")]
pub struct MemoizedPlutusData {
    original_bytes: Vec<u8>,
    // NOTE: This field isn't meant to be public, nor should we create any direct mutable
    // references to it. Reason being that this object is mostly meant to be read-only, and any
    // change to the 'data' should be reflected onto the 'original_bytes'.
    data: PlutusData,
}

impl MemoizedPlutusData {
    pub fn new(data: PlutusData) -> Result<Self, String> {
        let mut original_bytes = Vec::new();
        cbor::encode(&data, &mut original_bytes).map_err(|_| "failed to encode PlutusData".to_string())?;

        Ok(Self { original_bytes, data })
    }

    pub fn original_bytes(&self) -> &[u8] {
        &self.original_bytes
    }

    pub fn hash(&self) -> Hash<32> {
        Hasher::<256>::hash(&self.original_bytes)
    }
}

impl AsRef<PlutusData> for MemoizedPlutusData {
    fn as_ref(&self) -> &PlutusData {
        &self.data
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
        Ok(Self { data, original_bytes: original_bytes.to_vec() })
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

#[cfg(any(test, feature = "test-utils"))]
pub use tests::*;

#[cfg(any(test, feature = "test-utils"))]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::{
        PlutusData,
        plutus_data::{BigInt, BoundedBytes, Constr, VariableEncodingConstr, any_bigint, any_bounded_bytes},
        utils::cbor::{CborArray, CborMap},
    };

    // ---------------------------------------------------------------------------------------------
    // VariableEncodingPlutusData
    // ---------------------------------------------------------------------------------------------

    // NOTE: We do not use Pallas' PlutusData because it doesn't respect the
    // encoding expressed by the types for Map, but forces definite encoding.
    #[derive(Debug, Clone)]
    pub enum VariableEncodingPlutusData {
        Constr(VariableEncodingConstr<VariableEncodingPlutusData>),
        Map(CborMap<VariableEncodingPlutusData, VariableEncodingPlutusData>),
        BigInt(BigInt),
        BoundedBytes(BoundedBytes),
        Array(CborArray<VariableEncodingPlutusData>),
    }

    impl TryFrom<VariableEncodingPlutusData> for PlutusData {
        type Error = ();

        fn try_from(data: VariableEncodingPlutusData) -> Result<Self, Self::Error> {
            Ok(match data {
                VariableEncodingPlutusData::BigInt(i) => Self::BigInt(i),
                VariableEncodingPlutusData::BoundedBytes(i) => Self::BoundedBytes(i),
                VariableEncodingPlutusData::Array(xs) => Self::Array(match xs {
                    CborArray::Def(xs) | CborArray::Indef(xs) => {
                        xs.into_iter().map(|x| x.try_into()).collect::<Result<_, _>>()?
                    }
                }),
                VariableEncodingPlutusData::Map(xs) => Self::Map(match xs {
                    CborMap::Def(xs) | CborMap::Indef(xs) => xs
                        .into_iter()
                        .map(|(k, v)| k.try_into().and_then(|k| v.try_into().map(|v| (k, v))))
                        .collect::<Result<Vec<_>, _>>()?,
                }),
                VariableEncodingPlutusData::Constr(VariableEncodingConstr { tag, any_constructor, fields }) => {
                    Self::Constr(Constr {
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
                    e.encode_with(a, ctx)?;
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

    // ---------------------------------------------------------------------------------------------
    // Tests
    // ---------------------------------------------------------------------------------------------

    #[cfg(test)]
    mod internal {
        use proptest::prelude::*;

        use super::VariableEncodingPlutusData;
        use crate::{MemoizedPlutusData, PlutusData, from_cbor, to_cbor};

        proptest! {
            #[test]
            fn roundtrip_hex_encoded_str(original_data in VariableEncodingPlutusData::any(3)) {
                let original_bytes = to_cbor(&original_data);
                let result = MemoizedPlutusData::try_from(hex::encode(&original_bytes)).unwrap();

                assert_eq!(Some(result.as_ref()), PlutusData::try_from(original_data).ok().as_ref());
                assert_eq!(result.original_bytes(), &original_bytes);
            }
        }

        proptest! {
            #[test]
            fn roundtrip_cbor(original_data in VariableEncodingPlutusData::any(3)) {
                let original_bytes = to_cbor(&original_data);
                let result: MemoizedPlutusData = from_cbor(&original_bytes).unwrap();

                assert_eq!(Some(result.as_ref()), PlutusData::try_from(original_data).ok().as_ref());
                assert_eq!(result.original_bytes(), &original_bytes);
            }
        }

        #[test]
        fn invalid_string() {
            assert!(MemoizedPlutusData::try_from("foo".to_string()).is_err());
        }
    }
}
