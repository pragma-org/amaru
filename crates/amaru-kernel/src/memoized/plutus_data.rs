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

use crate::{cbor, memoized::blanket_try_from_hex_bytes, Bytes, Hash, Hasher, KeepRaw, PlutusData};
use pallas_codec::minicbor;

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(try_from = "&str")]
#[serde(into = "String")]
pub struct MemoizedPlutusData {
    original_bytes: Bytes,
    // NOTE: This field isn't meant to be public, nor should we create any direct mutable
    // references to it. Reason being that this object is mostly meant to be read-only, and any
    // change to the 'data' should be reflected onto the 'original_bytes'.
    data: PlutusData,
}

impl MemoizedPlutusData {
    pub fn new(data: PlutusData) -> Result<Self, String> {
        let mut buf = Vec::new();
        minicbor::encode(&data, &mut buf).map_err(|_| "failed to encode PlutusData".to_string())?;

        Ok(Self {
            original_bytes: Bytes::from(buf),
            data,
        })
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

impl<'b> From<KeepRaw<'b, PlutusData>> for MemoizedPlutusData {
    fn from(data: KeepRaw<'b, PlutusData>) -> Self {
        Self {
            original_bytes: Bytes::from(data.raw_cbor().to_vec()),
            data: data.unwrap(),
        }
    }
}

impl TryFrom<&str> for MemoizedPlutusData {
    type Error = String;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        blanket_try_from_hex_bytes(s, |original_bytes, data| Self {
            original_bytes,
            data,
        })
    }
}

impl TryFrom<String> for MemoizedPlutusData {
    type Error = String;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::try_from(s.as_str())
    }
}

impl TryFrom<Vec<u8>> for MemoizedPlutusData {
    type Error = cbor::decode::Error;

    fn try_from(bytes: Vec<u8>) -> Result<Self, Self::Error> {
        let data = cbor::decode(&bytes)?;
        Ok(Self {
            original_bytes: Bytes::from(bytes),
            data,
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::{to_cbor, Constr, KeyValuePairs, MaybeIndefArray};
    use pallas_primitives::{self as pallas, BigInt, BoundedBytes};
    use proptest::{prelude::*, strategy::Just};

    // NOTE: We do not use Pallas' PlutusData because it doesn't respect the
    // encoding expressed by the types for Map, but forces definite encoding.
    #[derive(Debug, Clone)]
    enum PlutusData {
        Constr(Constr<PlutusData>),
        Map(KeyValuePairs<PlutusData, PlutusData>),
        BigInt(BigInt),
        BoundedBytes(BoundedBytes),
        Array(MaybeIndefArray<PlutusData>),
    }
    impl From<PlutusData> for pallas::PlutusData {
        fn from(data: PlutusData) -> Self {
            match data {
                PlutusData::BigInt(i) => Self::BigInt(i),
                PlutusData::BoundedBytes(i) => Self::BoundedBytes(i),
                PlutusData::Array(xs) => Self::Array(match xs {
                    MaybeIndefArray::Def(xs) => {
                        MaybeIndefArray::Def(xs.into_iter().map(|x| x.into()).collect())
                    }
                    MaybeIndefArray::Indef(xs) => {
                        MaybeIndefArray::Indef(xs.into_iter().map(|x| x.into()).collect())
                    }
                }),
                PlutusData::Map(xs) => Self::Map(match xs {
                    KeyValuePairs::Def(xs) => KeyValuePairs::Def(
                        xs.into_iter().map(|(k, v)| (k.into(), v.into())).collect(),
                    ),
                    KeyValuePairs::Indef(xs) => KeyValuePairs::Indef(
                        xs.into_iter().map(|(k, v)| (k.into(), v.into())).collect(),
                    ),
                }),
                PlutusData::Constr(Constr {
                    tag,
                    any_constructor,
                    fields,
                }) => Self::Constr(Constr {
                    tag,
                    any_constructor,
                    fields: match fields {
                        MaybeIndefArray::Def(xs) => {
                            MaybeIndefArray::Def(xs.into_iter().map(|x| x.into()).collect())
                        }
                        MaybeIndefArray::Indef(xs) => {
                            MaybeIndefArray::Indef(xs.into_iter().map(|x| x.into()).collect())
                        }
                    },
                }),
            }
        }
    }

    impl<C> cbor::encode::Encode<C> for PlutusData {
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

    prop_compose! {
        pub(crate) fn any_bounded_bytes()(
            bytes in any::<Vec<u8>>(),
        ) -> BoundedBytes {
            BoundedBytes::from(bytes)
        }
    }

    pub(crate) fn any_bigint() -> impl Strategy<Value = BigInt> {
        prop_oneof![
            any::<i64>().prop_map(|i| BigInt::Int(i.into())),
            any_bounded_bytes().prop_map(BigInt::BigUInt),
            any_bounded_bytes().prop_map(BigInt::BigNInt),
        ]
    }

    fn any_constr(depth: u8) -> impl Strategy<Value = Constr<PlutusData>> {
        let any_constr_tag = prop_oneof![
            (Just(102), any::<u64>().prop_map(Some)),
            (121_u64..=127, Just(None)),
            (1280_u64..=1400, Just(None))
        ];

        let any_fields = prop::collection::vec(any_plutus_data(depth - 1), 0..depth as usize);

        (any_constr_tag, any_fields, any::<bool>()).prop_map(
            |((tag, any_constructor), fields, is_def)| Constr {
                tag,
                any_constructor,
                fields: if is_def {
                    MaybeIndefArray::Def(fields)
                } else {
                    MaybeIndefArray::Indef(fields)
                },
            },
        )
    }

    fn any_plutus_data(depth: u8) -> BoxedStrategy<PlutusData> {
        let int = any_bigint().prop_map(PlutusData::BigInt);

        let bytes = any_bounded_bytes().prop_map(PlutusData::BoundedBytes);

        if depth > 0 {
            let constr = any_constr(depth).prop_map(PlutusData::Constr);

            let array = (
                any::<bool>(),
                prop::collection::vec(any_plutus_data(depth - 1), 0..depth as usize),
            )
                .prop_map(|(is_def, xs)| {
                    PlutusData::Array(if is_def {
                        MaybeIndefArray::Def(xs)
                    } else {
                        MaybeIndefArray::Indef(xs)
                    })
                });

            let map = (
                any::<bool>(),
                prop::collection::vec(
                    (any_plutus_data(depth - 1), any_plutus_data(depth - 1)),
                    0..depth as usize,
                ),
            )
                .prop_map(|(is_def, kvs)| {
                    PlutusData::Map(if is_def {
                        KeyValuePairs::Def(kvs)
                    } else {
                        KeyValuePairs::Indef(kvs)
                    })
                });

            prop_oneof![int, bytes, constr, array, map].boxed()
        } else {
            prop_oneof![int, bytes].boxed()
        }
    }

    proptest! {
        #[test]
        fn roundtrip_hex_encoded_str(original_data in any_plutus_data(3)) {
            let original_bytes = to_cbor(&original_data);
            let result = MemoizedPlutusData::try_from(hex::encode(&original_bytes)).unwrap();

            assert_eq!(result.as_ref(), &pallas::PlutusData::from(original_data));
            assert_eq!(result.original_bytes(), &original_bytes);
        }
    }

    proptest! {
        #[test]
        fn roundtrip_cbor(original_data in any_plutus_data(3)) {
            let original_bytes = to_cbor(&original_data);
            let raw: KeepRaw<'_, pallas::PlutusData> = cbor::decode(&original_bytes).unwrap();
            let result: MemoizedPlutusData = raw.into();

            assert_eq!(result.as_ref(), &pallas::PlutusData::from(original_data));
            assert_eq!(result.original_bytes(), &original_bytes);
        }
    }

    #[test]
    fn invalid_string() {
        assert!(MemoizedPlutusData::try_from("foo".to_string()).is_err());
    }
}
