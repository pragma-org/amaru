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

use crate::{MemoizedTransactionOutput, TransactionInput, cbor};

// ----------------------------------------------------------------------------------- Generic utils

pub trait HasProxy: From<Self::Proxy> {
    type Proxy;
}

pub fn deserialize_map_proxy<'de, K, V, D>(deserializer: D) -> Result<BTreeMap<K, V>, D::Error>
where
    D: serde::Deserializer<'de>,
    K: Ord + HasProxy,
    K::Proxy: serde::Deserialize<'de>,
    V: HasProxy,
    V::Proxy: serde::Deserialize<'de>,
{
    let entries: Vec<(K::Proxy, V::Proxy)> = serde::Deserialize::deserialize(deserializer)?;
    Ok(entries.into_iter().map(|(k, v)| (K::from(k), V::from(v))).collect())
}

pub fn deserialize_option_proxy<'de, T, D>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: HasProxy,
    T::Proxy: serde::Deserialize<'de>,
{
    let option: Option<T::Proxy> = serde::Deserialize::deserialize(deserializer)?;
    Ok(option.map(T::from))
}

pub fn deserialize_proxy<'de, T, D>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: HasProxy,
    T::Proxy: serde::Deserialize<'de>,
{
    let proxy: T::Proxy = serde::Deserialize::deserialize(deserializer)?;
    Ok(T::from(proxy))
}

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

// -------------------------------------------------------------------------------- TransactionInput

impl HasProxy for TransactionInput {
    // NOTE: TranscationInput already defines a serde::Deserialize instance. The trait 'From' is
    // also reflexive, so this works.
    type Proxy = TransactionInput;
}

// ------------------------------------------------------------------------------- TransactionOutput

impl HasProxy for MemoizedTransactionOutput {
    type Proxy = MemoizedTransactionOutput;
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

    /// Resolve to `T` and then convert to `U` via `From<T>`. Convenience for fixture
    /// fields that deserialize through a proxy type but the user-facing value is the
    /// proxy's target (e.g. `RefOrInline<EraHistoryProxy>` → `EraHistory`).
    pub fn resolve_into<U: From<T>>(self, resolver: &impl RefResolver) -> Result<U, RefResolveError> {
        Ok(self.resolve(resolver)?.into())
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
