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

use crate::cbor;

/// A struct that maintains a reference to whether a cbor map was indef or not. Useful to
/// specifically test behavior around definite and indefinite CBOR encoded structures.
#[derive(Debug, Clone)]
pub enum CborMap<K, V> {
    Def(Vec<(K, V)>),
    Indef(Vec<(K, V)>),
}

impl<K, V> From<CborMap<K, V>> for Vec<(K, V)> {
    fn from(map: CborMap<K, V>) -> Self {
        match map {
            CborMap::Def(kvs) => kvs,
            CborMap::Indef(kvs) => kvs,
        }
    }
}

impl<K, V> Deref for CborMap<K, V> {
    type Target = Vec<(K, V)>;
    fn deref(&self) -> &Self::Target {
        match self {
            Self::Def(vec) => vec,
            Self::Indef(vec) => vec,
        }
    }
}

impl<C, K, V> cbor::encode::Encode<C> for CborMap<K, V>
where
    K: cbor::encode::Encode<C>,
    V: cbor::encode::Encode<C>,
{
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        match self {
            Self::Def(elems) => {
                e.map(elems.len() as u64)?;
                for (k, v) in elems.iter() {
                    e.encode_with(k, ctx)?;
                    e.encode_with(v, ctx)?;
                }
            }
            Self::Indef(elems) => {
                e.begin_map()?;
                for (k, v) in elems.iter() {
                    e.encode_with(k, ctx)?;
                    e.encode_with(v, ctx)?;
                }
                e.end()?;
            }
        };
        Ok(())
    }
}
