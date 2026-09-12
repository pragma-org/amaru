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

use std::ops::Deref;

use crate::{cbor, tee, to_cbor};

/// Decode an element and retain its original bytes.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, serde::Deserialize, serde::Serialize)]
pub struct WithOriginalBytes<A> {
    value: A,
    #[serde(with = "serde_bytes")]
    bytes: Vec<u8>,
}

/// JSON/human-readable as lowercase hex, binary (cbor4ii) as a CBOR byte string.
mod serde_bytes {
    use std::fmt;

    use serde::{
        Deserializer, Serializer,
        de::{self, Visitor},
    };

    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        if serializer.is_human_readable() {
            serializer.serialize_str(&hex::encode(bytes))
        } else {
            serializer.serialize_bytes(bytes)
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        deserializer.deserialize_any(BytesVisitor)
    }

    struct BytesVisitor;

    impl<'de> Visitor<'de> for BytesVisitor {
        type Value = Vec<u8>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a hex string, a byte string, or a sequence of bytes")
        }

        fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
            hex::decode(v).map_err(E::custom)
        }

        fn visit_bytes<E: de::Error>(self, v: &[u8]) -> Result<Self::Value, E> {
            Ok(v.to_vec())
        }

        fn visit_byte_buf<E: de::Error>(self, v: Vec<u8>) -> Result<Self::Value, E> {
            Ok(v)
        }

        fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
        where
            A: de::SeqAccess<'de>,
        {
            let mut bytes = Vec::with_capacity(seq.size_hint().unwrap_or(0));
            while let Some(b) = seq.next_element::<u8>()? {
                bytes.push(b);
            }
            Ok(bytes)
        }
    }
}

impl<A: Default + cbor::encode::Encode<()>> Default for WithOriginalBytes<A> {
    fn default() -> Self {
        Self::new(A::default())
    }
}

impl<A> WithOriginalBytes<A> {
    /// Returns `true` if the len is null.
    pub fn new(value: A) -> Self
    where
        A: cbor::encode::Encode<()>,
    {
        Self { bytes: to_cbor(&value), value }
    }

    /// Returns `true` if the len is null.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns the original serialised length for this element.
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    /// Consume the `WithOriginalBytes` wrapper to get back the element.
    pub fn into_inner(self) -> A {
        self.value
    }
}

impl<A: cbor::encode::Encode<()>> From<A> for WithOriginalBytes<A> {
    fn from(value: A) -> Self {
        Self::new(value)
    }
}

impl<A> Deref for WithOriginalBytes<A> {
    type Target = A;
    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

impl<'d, A: cbor::decode::Decode<'d, C>, C> cbor::decode::Decode<'d, C> for WithOriginalBytes<A> {
    fn decode(d: &mut cbor::Decoder<'d>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        let (value, bytes) = tee(d, |d| d.decode_with(ctx))?;
        Ok(WithOriginalBytes { bytes: bytes.to_vec(), value })
    }
}

impl<A: cbor::encode::Encode<C>, C> cbor::encode::Encode<C> for WithOriginalBytes<A> {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        _ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.writer_mut().write_all(&self.bytes).map_err(cbor::encode::Error::write)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::WithOriginalBytes;
    use crate::{from_cbor_no_leftovers, to_cbor};

    #[test]
    fn preserves_original_cbor_encoding() {
        // 1 fits in the CBOR header byte, so re-encoding would shrink these two bytes to one.
        let original = [0x18, 0x01];
        let value: WithOriginalBytes<u64> = from_cbor_no_leftovers(&original).expect("decode original CBOR");
        assert_eq!(*value, 1);
        assert_eq!(to_cbor(&value), original);
    }

    #[test]
    fn original_bytes_json_is_hex_string_and_cbor_is_byte_string() {
        let value = WithOriginalBytes::new(1u64);
        let json = serde_json::to_value(&value).expect("json");
        assert!(json["bytes"].as_str().is_some(), "{json}");
        assert!(json["bytes"].as_array().is_none());
        assert_eq!(&serde_json::from_value::<WithOriginalBytes<u64>>(json).expect("parse json"), &value);

        let mut buf = Vec::new();
        cbor4ii::serde::to_writer(&mut buf, &value).expect("cbor");
        let decoded: WithOriginalBytes<u64> = cbor4ii::serde::from_slice(&buf).expect("decode");
        assert_eq!(decoded, value);
    }

    proptest! {
        #[test]
        fn preserves_fixed_width_integer_encodings(n in any::<u64>()) {
            for original in fixed_width_encodings(n) {
                let value: WithOriginalBytes<u64> =
                    from_cbor_no_leftovers(&original).expect("decode fixed-width CBOR");
                prop_assert_eq!(*value, n);
                prop_assert_eq!(to_cbor(&value), original);
            }
        }
    }

    /// Every fixed-width CBOR encoding of `n`, widest first. All but the last one are wider than
    /// necessary, so an implementation that re-encodes instead of replaying the original bytes
    /// produces something shorter.
    fn fixed_width_encodings(n: u64) -> Vec<Vec<u8>> {
        let mut encodings = vec![[&[0x1b][..], &n.to_be_bytes()[..]].concat()];
        if let Ok(n) = u32::try_from(n) {
            encodings.push([&[0x1a][..], &n.to_be_bytes()[..]].concat());
        }
        if let Ok(n) = u16::try_from(n) {
            encodings.push([&[0x19][..], &n.to_be_bytes()[..]].concat());
        }
        if let Ok(n) = u8::try_from(n) {
            encodings.push([&[0x18][..], &n.to_be_bytes()[..]].concat());
        }
        encodings
    }
}
