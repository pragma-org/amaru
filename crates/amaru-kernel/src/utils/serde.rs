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

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::{MemoizedTransactionOutput, TransactionInput, cbor, from_cbor_no_leftovers};

// ----------------------------------------------------------------------------------- Generic utils

pub fn hex_to_bytes<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s: String = serde::Deserialize::deserialize(deserializer)?;
    hex::decode(s).map_err(serde::de::Error::custom)
}

/// Decode a fixture-shape UTxO list — `[{ "input": "<cbor-hex>", "output": "<cbor-hex>" }]` —
/// into a `BTreeMap<TransactionInput, MemoizedTransactionOutput>` by hex-decoding then
/// CBOR-decoding each entry.
pub fn deserialize_utxo<'de, D>(
    deserializer: D,
) -> Result<BTreeMap<TransactionInput, MemoizedTransactionOutput>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(serde::Deserialize)]
    struct UtxoEntryProxy {
        #[serde(deserialize_with = "hex_to_bytes")]
        input: Vec<u8>,
        #[serde(deserialize_with = "hex_to_bytes")]
        output: Vec<u8>,
    }

    let entries: Vec<UtxoEntryProxy> = serde::Deserialize::deserialize(deserializer)?;
    entries
        .into_iter()
        .map(|entry| {
            let input: TransactionInput = cbor::decode(&entry.input).map_err(serde::de::Error::custom)?;
            let output: MemoizedTransactionOutput = cbor::decode(&entry.output).map_err(serde::de::Error::custom)?;
            Ok((input, output))
        })
        .collect()
}

// --------------------------------------------------------- Derive serde instance from minicbor ones

pub struct SerdeUsingCbor<T>(pub T);

impl<T> From<T> for SerdeUsingCbor<T> {
    fn from(t: T) -> Self {
        Self(t)
    }
}

impl<T: cbor::Encode<()>> serde::Serialize for SerdeUsingCbor<T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serialize_using_cbor(&self.0, serializer)
    }
}

pub fn serialize_using_cbor<S, T>(value: &T, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
    T: cbor::Encode<()>,
{
    let bytes = cbor::to_cbor(value);

    if serializer.is_human_readable() {
        serializer.serialize_str(&hex::encode(bytes))
    } else {
        serializer.serialize_bytes(&bytes)
    }
}

impl<'de, T: for<'d> cbor::Decode<'d, ()>> serde::Deserialize<'de> for SerdeUsingCbor<T> {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        deserialize_using_cbor(d).map(SerdeUsingCbor)
    }
}

pub fn deserialize_using_cbor<'de, T, D>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: for<'d> cbor::Decode<'d, ()>,
{
    if deserializer.is_human_readable() {
        let encoded = String::deserialize(deserializer)?;
        from_cbor_no_leftovers(&hex::decode(encoded).map_err(serde::de::Error::custom)?)
    } else {
        from_cbor_no_leftovers(<&[u8]>::deserialize(deserializer)?)
    }
    .map_err(serde::de::Error::custom)
}

// ----------------------------------------------------------------------------------- RefOrInline

/// A JSON value that's either embedded inline or a reference to another document.
///
/// Inline form: any shape `T` accepts. Reference form: an object with `$ref` (and
/// optionally `$override`). `$override` is shallow-merged over the referenced
/// document before final deserialization as `T`, allowing one-off variations on a
/// shared canonical document without materializing a separate file for every variant.
///
/// `Deserialize` is pure: it classifies the JSON shape and stores the reference path
/// verbatim. The actual fetch happens later in `resolve`, which delegates I/O to a
/// `RefResolver` supplied by the caller.
pub enum RefOrInline<T> {
    Inline(T),
    Ref { path: String, override_: Option<crate::json::Value> },
}

impl<'de, T: serde::de::DeserializeOwned> serde::Deserialize<'de> for RefOrInline<T> {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let value = crate::json::Value::deserialize(d)?;
        if let crate::json::Value::Object(ref obj) = value
            && let Some(ref_val) = obj.get("$ref")
        {
            let path = ref_val.as_str().ok_or_else(|| serde::de::Error::custom("$ref must be a string"))?.to_string();
            let override_ = obj.get("$override").cloned();
            return Ok(RefOrInline::Ref { path, override_ });
        }
        crate::json::from_value(value).map(RefOrInline::Inline).map_err(serde::de::Error::custom)
    }
}

impl<T: serde::de::DeserializeOwned> RefOrInline<T> {
    /// Resolve to `T`. For the inline form this is a no-op; for the reference form
    /// this asks `resolver` for the referenced JSON, applies any `$override`, and
    /// deserializes the result as `T`.
    pub fn resolve(self, resolver: &impl RefResolver) -> Result<T, RefResolveError> {
        match self {
            RefOrInline::Inline(t) => Ok(t),
            RefOrInline::Ref { path, override_ } => {
                let mut value = resolver
                    .resolve(&path)
                    .map_err(|source| RefResolveError::Resolver { path: path.clone(), source })?;
                if let Some(o) = override_ {
                    shallow_merge(&mut value, o);
                }
                crate::json::from_value(value).map_err(RefResolveError::Deserialize)
            }
        }
    }
}

/// Resolves `$ref` path strings to JSON values. Implementations carry whatever
/// effect they need (filesystem I/O, in-memory lookups, hash verification, etc.);
/// the trait surface is intentionally minimal.
pub trait RefResolver {
    fn resolve(&self, path: &str) -> Result<crate::json::Value, Box<dyn std::error::Error + Send + Sync>>;
}

#[derive(Debug, thiserror::Error)]
pub enum RefResolveError {
    #[error("ref resolver failed for {path}: {source}")]
    Resolver {
        path: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    #[error("failed to deserialize resolved JSON: {0}")]
    Deserialize(#[from] crate::json::Error),
}

fn shallow_merge(base: &mut crate::json::Value, over: crate::json::Value) {
    let taken = std::mem::replace(base, crate::json::Value::Null);
    *base = match (taken, over) {
        (crate::json::Value::Object(mut base_map), crate::json::Value::Object(over_map)) => {
            for (k, v) in over_map {
                base_map.insert(k, v);
            }
            crate::json::Value::Object(base_map)
        }
        (_, over) => over,
    };
}

// --------------------------------------------------------------------------- FilesystemRefResolver

/// Resolves `$ref` paths against a base directory on disk. `$ref` strings are
/// joined onto `base_dir`; the referenced file is read and parsed as JSON.
#[cfg(any(test, feature = "test-utils"))]
pub struct FilesystemRefResolver {
    base_dir: std::path::PathBuf,
}

#[cfg(any(test, feature = "test-utils"))]
impl FilesystemRefResolver {
    pub fn new(base_dir: impl Into<std::path::PathBuf>) -> Self {
        Self { base_dir: base_dir.into() }
    }
}

#[cfg(any(test, feature = "test-utils"))]
impl RefResolver for FilesystemRefResolver {
    fn resolve(&self, path: &str) -> Result<crate::json::Value, Box<dyn std::error::Error + Send + Sync>> {
        let full = self.base_dir.join(path);
        let raw = std::fs::read_to_string(&full).map_err(|e| format!("read {}: {e}", full.display()))?;
        let value: crate::json::Value =
            crate::json::from_str(&raw).map_err(|e| format!("parse {}: {e}", full.display()))?;
        Ok(value)
    }
}

// ---------------------------------------------------------------------- Opaque byte blobs (serde)

/// Serde for opaque byte blobs: lowercase hex when human-readable (JSON), a CBOR byte string
/// otherwise (cbor4ii). Deserialize accepts hex, a byte string, or a sequence of `u8` so older
/// integer-array encodings still load.
///
/// Use with `#[serde(with = "amaru_kernel::utils::serde::bytes")]` on a `Vec<u8>` field, or call
/// [`serialize`] / [`deserialize`] from a type's `Serialize` / `Deserialize` impls.
pub mod bytes {
    use std::{fmt, sync::Arc};

    use serde::{
        Deserializer, Serializer,
        de::{self, Visitor},
    };

    /// JSON Schema for a lowercase hex string of whole bytes (`^([0-9a-f]{2})*$`).
    pub fn json_schema(description: &'static str) -> schemars::schema::Schema {
        #[allow(clippy::expect_used)]
        serde_json::from_value(serde_json::json!({
            "type": "string",
            "pattern": "^([0-9a-f]{2})*$",
            "description": description
        }))
        .expect("hex bytes json schema is valid")
    }

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

    /// `#[serde(with = "amaru_kernel::utils::serde::arc_bytes")]` for `Arc<[u8]>` fields.
    pub fn deserialize_arc<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Arc<[u8]>, D::Error> {
        deserialize(deserializer).map(Arc::from)
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

    #[cfg(test)]
    pub(crate) fn assert_json_hex_and_cbor_bstr<T>(value: &T, payload: &[u8])
    where
        T: serde::Serialize + for<'de> serde::Deserialize<'de> + PartialEq + fmt::Debug,
    {
        let json = serde_json::to_string(value).expect("json");
        assert_eq!(json, format!("\"{}\"", hex::encode(payload)));
        assert_eq!(&serde_json::from_str::<T>(&json).expect("parse hex json"), value);

        let array_json = serde_json::to_string(payload).expect("array json");
        assert_eq!(&serde_json::from_str::<T>(&array_json).expect("parse integer-array json"), value);

        let mut buf = Vec::new();
        cbor4ii::serde::to_writer(&mut buf, value).expect("cbor");
        assert_eq!(buf.first().map(|b| b >> 5), Some(2), "CBOR major type 2 (byte string), got {buf:x?}");
        assert_eq!(&cbor4ii::serde::from_slice::<T>(&buf).expect("parse cbor bstr"), value);

        let mut old = Vec::new();
        let as_seq = payload.to_vec();
        cbor4ii::serde::to_writer(&mut old, &as_seq).expect("old cbor array");
        assert_eq!(&cbor4ii::serde::from_slice::<T>(&old).expect("parse cbor integer array"), value);
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
        #[serde(transparent)]
        struct Blob(#[serde(with = "super")] Vec<u8>);

        #[test]
        fn json_is_hex_string_and_cbor_is_byte_string() {
            let payload = [0xabu8, 0xcd, 0xef];
            assert_json_hex_and_cbor_bstr(&Blob(payload.to_vec()), &payload);
        }

        #[test]
        fn empty_blob_roundtrips() {
            assert_json_hex_and_cbor_bstr(&Blob(Vec::new()), &[]);
        }

        #[test]
        fn json_schema_is_hex_string() {
            let schema = serde_json::to_value(json_schema("hex-encoded bytes")).expect("schema");
            assert_eq!(schema["type"], "string");
            assert_eq!(schema["pattern"], "^([0-9a-f]{2})*$");
        }
    }
}
