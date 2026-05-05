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

use crate::{Bytes, Value, cbor, from_cbor};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoizedValue {
    original_bytes: Bytes,
    // NOTE: This field isn't meant to be public, nor should we create any direct mutable
    // references to it. Reason being that this object is mostly meant to be read-only, and any
    // change to the 'value' should be reflected onto the 'original_bytes'.
    value: Value,
}

impl MemoizedValue {
    pub fn new(value: Value) -> Result<Self, String> {
        let mut buf = Vec::new();
        cbor::encode(&value, &mut buf).map_err(|_| "failed to encode Value".to_string())?;

        Ok(Self { original_bytes: Bytes::from(buf), value })
    }

    pub fn original_bytes(&self) -> &[u8] {
        &self.original_bytes
    }
}

impl AsRef<Value> for MemoizedValue {
    fn as_ref(&self) -> &Value {
        &self.value
    }
}

impl TryFrom<Bytes> for MemoizedValue {
    type Error = String;

    fn try_from(original_bytes: Bytes) -> Result<Self, Self::Error> {
        let value = from_cbor(&original_bytes).ok_or_else(|| "invalid serialized value".to_string())?;

        Ok(Self { original_bytes, value })
    }
}

impl<'b, C> cbor::Decode<'b, C> for MemoizedValue {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        let start_pos = d.position();
        let value: Value = d.decode_with(ctx)?;
        let end_pos = d.position();
        let original_bytes = Bytes::from(d.input()[start_pos..end_pos].to_vec());

        Ok(Self { original_bytes, value })
    }
}

impl<C> cbor::Encode<C> for MemoizedValue {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        _ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.writer_mut().write_all(&self.original_bytes[..]).map_err(cbor::encode::Error::write)
    }
}

#[cfg(test)]
mod tests {
    use pallas_primitives::{
        Bytes as PallasBytes, NonEmptyKeyValuePairs, PositiveCoin, conway::Multiasset as PallasMultiasset,
    };
    use proptest::prelude::*;

    use super::*;
    use crate::{Hash, size::CREDENTIAL, to_cbor};

    fn any_coin() -> impl Strategy<Value = u64> {
        any::<u64>()
    }

    fn any_policy() -> impl Strategy<Value = Hash<CREDENTIAL>> {
        any::<[u8; 28]>().prop_map(Hash::from)
    }

    fn any_asset_name() -> impl Strategy<Value = PallasBytes> {
        prop::collection::vec(any::<u8>(), 0..32).prop_map(PallasBytes::from)
    }

    fn any_positive_coin() -> impl Strategy<Value = PositiveCoin> {
        (1u64..u64::MAX).prop_map(|n| PositiveCoin::try_from(n).unwrap())
    }

    fn any_multiasset() -> impl Strategy<Value = PallasMultiasset<PositiveCoin>> {
        prop::collection::vec(
            (any_policy(), prop::collection::vec((any_asset_name(), any_positive_coin()), 1..4)),
            1..4,
        )
        .prop_map(|policies| {
            let pairs: Vec<_> = policies
                .into_iter()
                .map(|(policy, assets)| {
                    let assets = NonEmptyKeyValuePairs::try_from(assets).unwrap();
                    (policy, assets)
                })
                .collect();
            NonEmptyKeyValuePairs::try_from(pairs).unwrap()
        })
    }

    fn any_value() -> impl Strategy<Value = Value> {
        prop_oneof![
            any_coin().prop_map(Value::Coin),
            (any_coin(), any_multiasset()).prop_map(|(coin, ma)| Value::Multiasset(coin, ma)),
        ]
    }

    proptest! {
        #[test]
        fn roundtrip_cbor(original_value in any_value()) {
            let original_bytes = to_cbor(&original_value);
            let memoized = MemoizedValue::try_from(Bytes::from(original_bytes.clone())).unwrap();

            assert_eq!(memoized.as_ref(), &original_value);
            assert_eq!(memoized.original_bytes(), &original_bytes);
        }

        #[test]
        fn roundtrip_decode(original_value in any_value()) {
            let original_bytes = to_cbor(&original_value);
            let memoized: MemoizedValue = crate::from_cbor(&original_bytes).unwrap();

            assert_eq!(memoized.as_ref(), &original_value);
            assert_eq!(memoized.original_bytes(), &original_bytes);
        }

        #[test]
        fn new_matches_encoded(original_value in any_value()) {
            let memoized = MemoizedValue::new(original_value.clone()).unwrap();
            let direct = to_cbor(&original_value);

            assert_eq!(memoized.original_bytes(), &direct);
        }
    }
}
