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
