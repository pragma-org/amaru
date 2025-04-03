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

use crate::{Bytes, PostAlonzoTransactionOutput, TransactionInput, TransactionOutput, Value};
use std::collections::BTreeMap;

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

// -------------------------------------------------------------------------------- TransactionInput

impl HasProxy for TransactionInput {
    // NOTE: TranscationInput already defines a serde::Deserialize instance. The trait 'From' is
    // also reflexive, so this works.
    type Proxy = TransactionInput;
}

// ------------------------------------------------------------------------------- TransactionOutput

#[derive(Debug, serde::Deserialize)]
pub struct TransactionOutputProxy {
    address: Bytes,
    // TODO: expand this
}

impl HasProxy for TransactionOutput {
    type Proxy = TransactionOutputProxy;
}

impl From<TransactionOutputProxy> for TransactionOutput {
    fn from(proxy: TransactionOutputProxy) -> Self {
        Self::PostAlonzo(PostAlonzoTransactionOutput {
            address: proxy.address,
            value: Value::Coin(0),
            datum_option: None,
            script_ref: None,
        })
    }
}
