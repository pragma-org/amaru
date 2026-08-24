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

use std::{fmt, ops::Deref, str::FromStr};

use crate::cbor;

// -----------------------------------------------------------------------------
// Hash sizes
// -----------------------------------------------------------------------------

pub mod size {
    pub const BLOCK_BODY: usize = 32;

    pub const CREDENTIAL: usize = 28;

    pub const DATUM: usize = 32;

    pub const HEADER: usize = 32;

    pub const KEY: usize = CREDENTIAL;

    pub const NONCE: usize = 32;

    pub const POOL_COLD_KEY: usize = 28;

    pub const SCRIPT: usize = CREDENTIAL;

    pub const TRANSACTION_BODY: usize = 32;

    pub const VRF_KEY: usize = 32;
}

// -----------------------------------------------------------------------------
// Aliases
// -----------------------------------------------------------------------------

pub type HeaderHash = Hash<{ size::HEADER }>;

pub type PoolId = Hash<{ size::POOL_COLD_KEY }>;

// -----------------------------------------------------------------------------
// Constants
// -----------------------------------------------------------------------------

pub const NULL_HASH28: Hash<28> = Hash::new([0; 28]);

pub const NULL_HASH32: Hash<32> = Hash::new([0; 32]);

pub const ORIGIN_HASH: Hash<{ size::HEADER }> = NULL_HASH32;

// -----------------------------------------------------------------------------
// Hash
// -----------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, std::hash::Hash)]
pub struct Hash<const BYTES: usize>([u8; BYTES]);

impl<const BYTES: usize> Hash<BYTES> {
    #[inline]
    pub const fn new(bytes: [u8; BYTES]) -> Self {
        Self(bytes)
    }
}

impl<const BYTES: usize> From<[u8; BYTES]> for Hash<BYTES> {
    #[inline]
    fn from(bytes: [u8; BYTES]) -> Self {
        Self::new(bytes)
    }
}

impl<const BYTES: usize> From<&[u8]> for Hash<BYTES> {
    fn from(value: &[u8]) -> Self {
        let mut hash = [0; BYTES];
        hash.copy_from_slice(value);
        Self::new(hash)
    }
}

impl<const BYTES: usize> AsRef<[u8]> for Hash<BYTES> {
    #[inline]
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl<const BYTES: usize> Deref for Hash<BYTES> {
    type Target = [u8; BYTES];

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<const BYTES: usize> PartialEq<[u8]> for Hash<BYTES> {
    fn eq(&self, other: &[u8]) -> bool {
        self.0.eq(other)
    }
}

impl<const BYTES: usize> fmt::Debug for Hash<BYTES> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple(&format!("Hash<{BYTES}>")).field(&hex::encode(self)).finish()
    }
}

impl<const BYTES: usize> fmt::Display for Hash<BYTES> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&hex::encode(self))
    }
}

impl<const BYTES: usize> FromStr for Hash<BYTES> {
    type Err = hex::FromHexError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut bytes = [0; BYTES];
        hex::decode_to_slice(s, &mut bytes)?;
        Ok(Self::new(bytes))
    }
}

impl<C, const BYTES: usize> cbor::Encode<C> for Hash<BYTES> {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        _ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.bytes(&self.0)?.ok()
    }
}

impl<'a, C: cbor::HasProtocolVersion, const BYTES: usize> cbor::Decode<'a, C> for Hash<BYTES> {
    fn decode(d: &mut cbor::Decoder<'a>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        let bytes = cbor::decode_bytes_with(d, ctx)?;
        if bytes.len() == BYTES {
            let mut hash = [0; BYTES];
            hash.copy_from_slice(&bytes);
            Ok(Self::new(hash))
        } else {
            Err(cbor::decode::Error::message("invalid hash size"))
        }
    }
}

impl<const BYTES: usize> serde::Serialize for Hash<BYTES> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        if serializer.is_human_readable() {
            serializer.serialize_str(&self.to_string())
        } else {
            serializer.serialize_bytes(self.as_ref())
        }
    }
}

struct HashVisitor<const BYTES: usize> {}

impl<'de, const BYTES: usize> serde::de::Visitor<'de> for HashVisitor<BYTES> {
    type Value = Hash<BYTES>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "a {BYTES}-byte hash as a hex string or a byte string")
    }

    fn visit_str<E>(self, s: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Hash::<BYTES>::from_str(s).map_err(|_| serde::de::Error::invalid_value(serde::de::Unexpected::Str(s), &self))
    }

    fn visit_bytes<E>(self, v: &[u8]) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        try_from_slice(v).ok_or_else(|| serde::de::Error::invalid_length(v.len(), &self))
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::SeqAccess<'de>,
    {
        let mut bytes = [0u8; BYTES];
        for (i, slot) in bytes.iter_mut().enumerate() {
            *slot = seq.next_element()?.ok_or_else(|| serde::de::Error::invalid_length(i, &self))?;
        }
        if seq.next_element::<u8>()?.is_some() {
            return Err(serde::de::Error::invalid_length(BYTES + 1, &self));
        }
        Ok(Hash::new(bytes))
    }
}

impl<'de, const BYTES: usize> serde::Deserialize<'de> for Hash<BYTES> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(HashVisitor::<BYTES> {})
    }
}

/// JSON Schema for a lowercase hex string of `byte_len` bytes (`^[0-9a-f]{2*byte_len}$`).
///
/// Draft-07 `contentEncoding` is annotation-only and does not constrain characters.
pub(crate) fn hex_string_json_schema(byte_len: usize, description: &'static str) -> schemars::schema::Schema {
    let hex_len = byte_len * 2;
    #[allow(clippy::expect_used)]
    serde_json::from_value(serde_json::json!({
        "type": "string",
        "pattern": format!("^[0-9a-f]{{{hex_len}}}$"),
        "description": description
    }))
    .expect("hex string json schema is valid")
}

impl<const BYTES: usize> schemars::JsonSchema for Hash<BYTES> {
    fn schema_name() -> String {
        format!("Hash<{BYTES}>")
    }

    fn json_schema(_gen: &mut schemars::r#gen::SchemaGenerator) -> schemars::schema::Schema {
        hex_string_json_schema(BYTES, "hex-encoded hash")
    }

    fn is_referenceable() -> bool {
        false
    }
}

// -----------------------------------------------------------------------------
// Display
// -----------------------------------------------------------------------------

pub fn fmt<const N: usize>(hashes: &[Hash<N>]) -> String {
    let mut out = String::new();

    for (i, hash) in hashes.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }

        out.push_str(&format!("{hash}").as_str()[0..12])
    }

    out
}

pub fn try_from_slice<const N: usize>(slice: &[u8]) -> Option<Hash<N>> {
    if slice.len() != N {
        return None;
    }

    let mut sized = [0u8; N];
    sized.copy_from_slice(slice);
    Some(sized.into())
}

// -----------------------------------------------------------------------------
// Test
// -----------------------------------------------------------------------------

#[cfg(any(test, feature = "test-utils"))]
pub use tests::*;

#[cfg(any(test, feature = "test-utils"))]
mod tests {
    use proptest::prelude::*;

    use super::*;

    pub fn any_hash28() -> impl Strategy<Value = Hash<28>> {
        any::<[u8; 28]>().prop_map(Hash::from)
    }

    pub fn any_hash32() -> impl Strategy<Value = Hash<32>> {
        any::<[u8; 32]>().prop_map(Hash::from)
    }

    #[cfg(test)]
    mod serde_format {
        use super::*;

        #[test]
        fn json_is_hex_string() {
            let hash = Hash::<32>::from([0xabu8; 32]);
            let json = serde_json::to_string(&hash).expect("json");
            assert_eq!(json, format!("\"{hash}\""));
            assert_eq!(hash, serde_json::from_str::<Hash<32>>(&json).expect("parse"));
        }

        #[test]
        fn cbor_is_byte_string() {
            let hash = Hash::<32>::from([0xabu8; 32]);
            let mut buf = Vec::new();
            cbor4ii::serde::to_writer(&mut buf, &hash).expect("encode");
            assert!(buf.windows(2).any(|w| w == [0x58, 0x20]), "32-byte CBOR byte string header");
            let decoded: Hash<32> = cbor4ii::serde::from_slice(&buf).expect("decode");
            assert_eq!(decoded, hash);
        }

        #[test]
        fn json_schema_uses_lowercase_hex_pattern() {
            let schema32 = serde_json::to_value(schemars::schema_for!(Hash<32>).schema).expect("schema");
            assert_eq!(schema32["pattern"], "^[0-9a-f]{64}$");
            assert!(schema32.get("contentEncoding").is_none());
            assert!(schema32.get("minLength").is_none());
            assert!(schema32.get("maxLength").is_none());

            let schema28 = serde_json::to_value(schemars::schema_for!(Hash<28>).schema).expect("schema");
            assert_eq!(schema28["pattern"], "^[0-9a-f]{56}$");
        }
    }
}
