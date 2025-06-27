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

use crate::{from_cbor, Bytes, MemoizedTransactionOutput, TransactionInput};
use pallas_codec::utils::CborWrap;
use pallas_primitives::{
    conway::{DatumOption, Hash, ScriptRef},
    PlutusScript,
};
use std::{collections::BTreeMap, ops::Deref};

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
    Ok(entries
        .into_iter()
        .map(|(k, v)| (K::from(k), V::from(v)))
        .collect())
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

// -------------------------------------------------------------------------------- TransactionInput

impl HasProxy for TransactionInput {
    // NOTE: TranscationInput already defines a serde::Deserialize instance. The trait 'From' is
    // also reflexive, so this works.
    type Proxy = TransactionInput;
}

// ------------------------------------------------------------------------------- TransactionOutput

// Adding new fields here as an `Option` to not break existing context.json files. Can go back through and clean up later
#[derive(Debug, serde::Deserialize)]
pub struct TransactionOutputProxy {
    address: Bytes,
    // TODO: support value that is more than just lovelace
    value: Option<u64>,
    script_ref: Option<ScriptRefProxy>,
    datum: Option<DatumOptionProxy>,
    // TODO: expand this
}

impl HasProxy for MemoizedTransactionOutput {
    type Proxy = TransactionOutputProxy;
}
