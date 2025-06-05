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

use pallas_codec::utils::CborWrap;
use pallas_primitives::{
    conway::{DatumOption, Hash, ScriptRef},
    PlutusScript,
};

use crate::{
    from_cbor, Bytes, PostAlonzoTransactionOutput, TransactionInput, TransactionOutput, Value,
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

impl HasProxy for TransactionOutput {
    type Proxy = TransactionOutputProxy;
}

impl From<TransactionOutputProxy> for TransactionOutput {
    fn from(proxy: TransactionOutputProxy) -> Self {
        Self::PostAlonzo(PostAlonzoTransactionOutput {
            address: proxy.address,
            value: Value::Coin(proxy.value.unwrap_or_default()),
            datum_option: proxy.datum.map(DatumOption::from),
            script_ref: proxy
                .script_ref
                .map(|proxy| CborWrap(ScriptRef::from(proxy))),
        })
    }
}

// ------------------------------------------------------------------------------- ScriptRef

#[derive(Debug, serde::Deserialize)]
pub enum ScriptRefProxy {
    NativeScript(Bytes),
    PlutusV1(Bytes),
    PlutusV2(Bytes),
    PlutusV3(Bytes),
}

impl HasProxy for ScriptRef {
    type Proxy = ScriptRefProxy;
}

impl From<ScriptRefProxy> for ScriptRef {
    #[allow(clippy::unwrap_used)]
    fn from(value: ScriptRefProxy) -> Self {
        match value {
            ScriptRefProxy::NativeScript(bytes) => {
                // This code should only be run during tests, so a panic here is fine
                ScriptRef::NativeScript(from_cbor(bytes.deref()).unwrap())
            }
            ScriptRefProxy::PlutusV1(bytes) => ScriptRef::PlutusV1Script(PlutusScript::<1>(bytes)),
            ScriptRefProxy::PlutusV2(bytes) => ScriptRef::PlutusV2Script(PlutusScript::<2>(bytes)),
            ScriptRefProxy::PlutusV3(bytes) => ScriptRef::PlutusV3Script(PlutusScript::<3>(bytes)),
        }
    }
}

// ------------------------------------------------------------------------------- DatumOption

#[derive(Debug, serde::Deserialize)]
pub enum DatumOptionProxy {
    Hash(Bytes),
    Data(Bytes),
}

impl HasProxy for DatumOption {
    type Proxy = DatumOptionProxy;
}

impl From<DatumOptionProxy> for DatumOption {
    #[allow(clippy::unwrap_used)]
    fn from(value: DatumOptionProxy) -> Self {
        match value {
            DatumOptionProxy::Hash(bytes) => DatumOption::Hash(Hash::from(bytes.as_slice())),
            // This code should only be run during tests, so a panic here is fine
            DatumOptionProxy::Data(data) => {
                DatumOption::Data(CborWrap(from_cbor(data.deref()).unwrap()))
            }
        }
    }
}
