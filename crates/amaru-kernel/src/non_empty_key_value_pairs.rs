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

use crate::{
    Deref, KeyValuePairs,
    cbor::{Decode, Encode, decode::Error},
};
use serde::{Deserialize, Serialize};

/// Custom collection to ensure ordered pairs of values (non-empty)
///
/// Since the ordering of the entries requires a particular order to maintain
/// canonicalization for isomorphic decoding / encoding operators, we use a Vec
/// as the underlaying struct for storage of the items (as opposed to a BTreeMap
/// or HashMap).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(try_from = "Vec::<(K, V)>", into = "Vec::<(K, V)>")]
pub enum NonEmptyKeyValuePairs<K, V>
where
    K: Clone,
    V: Clone,
{
    Def(Vec<(K, V)>),
    Indef(Vec<(K, V)>),
}

impl<K, V> IntoIterator for NonEmptyKeyValuePairs<K, V>
where
    K: Clone,
    V: Clone,
{
    type Item = (K, V);
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        match self {
            NonEmptyKeyValuePairs::Def(pairs) => pairs.into_iter(),
            NonEmptyKeyValuePairs::Indef(pairs) => pairs.into_iter(),
        }
    }
}

impl<K, V> NonEmptyKeyValuePairs<K, V>
where
    K: Clone,
    V: Clone,
{
    pub fn to_vec(self) -> Vec<(K, V)> {
        self.into()
    }

    pub fn from_vec(x: Vec<(K, V)>) -> Option<Self> {
        if x.is_empty() {
            None
        } else {
            Some(NonEmptyKeyValuePairs::Def(x))
        }
    }

    pub fn from_pallas(pairs: pallas_primitives::NonEmptyKeyValuePairs<K, V>) -> Self {
        match pairs {
            pallas_primitives::NonEmptyKeyValuePairs::Def(v) => Self::Def(v),
            pallas_primitives::NonEmptyKeyValuePairs::Indef(v) => Self::Indef(v),
        }
    }

    pub fn to_pallas(self) -> pallas_primitives::NonEmptyKeyValuePairs<K, V> {
        match self {
            Self::Def(v) => pallas_primitives::NonEmptyKeyValuePairs::Def(v),
            Self::Indef(v) => pallas_primitives::NonEmptyKeyValuePairs::Indef(v),
        }
    }
}

impl<K, V> From<NonEmptyKeyValuePairs<K, V>> for Vec<(K, V)>
where
    K: Clone,
    V: Clone,
{
    fn from(other: NonEmptyKeyValuePairs<K, V>) -> Self {
        match other {
            NonEmptyKeyValuePairs::Def(x) => x,
            NonEmptyKeyValuePairs::Indef(x) => x,
        }
    }
}

impl<K, V> TryFrom<Vec<(K, V)>> for NonEmptyKeyValuePairs<K, V>
where
    K: Clone,
    V: Clone,
{
    type Error = String;

    fn try_from(value: Vec<(K, V)>) -> Result<Self, Self::Error> {
        if value.is_empty() {
            Err("NonEmptyKeyValuePairs must contain at least one element".into())
        } else {
            Ok(NonEmptyKeyValuePairs::Def(value))
        }
    }
}

impl<K, V> TryFrom<KeyValuePairs<K, V>> for NonEmptyKeyValuePairs<K, V>
where
    K: Clone,
    V: Clone,
{
    type Error = String;

    fn try_from(value: KeyValuePairs<K, V>) -> Result<Self, Self::Error> {
        match value {
            KeyValuePairs::Def(x) => {
                if x.is_empty() {
                    Err("NonEmptyKeyValuePairs must contain at least one element".into())
                } else {
                    Ok(NonEmptyKeyValuePairs::Def(x))
                }
            }
            KeyValuePairs::Indef(x) => {
                if x.is_empty() {
                    Err("NonEmptyKeyValuePairs must contain at least one element".into())
                } else {
                    Ok(NonEmptyKeyValuePairs::Indef(x))
                }
            }
        }
    }
}

impl<K, V> Deref for NonEmptyKeyValuePairs<K, V>
where
    K: Clone,
    V: Clone,
{
    type Target = Vec<(K, V)>;

    fn deref(&self) -> &Self::Target {
        match self {
            NonEmptyKeyValuePairs::Def(x) => x,
            NonEmptyKeyValuePairs::Indef(x) => x,
        }
    }
}

impl<'b, C, K, V> minicbor::decode::Decode<'b, C> for NonEmptyKeyValuePairs<K, V>
where
    K: Decode<'b, C> + Clone,
    V: Decode<'b, C> + Clone,
{
    #[allow(clippy::wildcard_enum_match_arm)]
    fn decode(d: &mut minicbor::Decoder<'b>, ctx: &mut C) -> Result<Self, minicbor::decode::Error> {
        let datatype = d.datatype()?;

        let items: Result<Vec<_>, _> = d.map_iter_with::<C, K, V>(ctx)?.collect();
        let items = items?;

        if items.is_empty() {
            return Err(Error::message(
                "decoding empty map as NonEmptyKeyValuePairs",
            ));
        }

        match datatype {
            minicbor::data::Type::Map => Ok(NonEmptyKeyValuePairs::Def(items)),
            minicbor::data::Type::MapIndef => Ok(NonEmptyKeyValuePairs::Indef(items)),
            _ => Err(minicbor::decode::Error::message(
                "invalid data type for nonemptykeyvaluepairs",
            )),
        }
    }
}

impl<C, K, V> minicbor::encode::Encode<C> for NonEmptyKeyValuePairs<K, V>
where
    K: Encode<C> + Clone,
    V: Encode<C> + Clone,
{
    fn encode<W: minicbor::encode::Write>(
        &self,
        e: &mut minicbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), minicbor::encode::Error<W::Error>> {
        match self {
            NonEmptyKeyValuePairs::Def(x) => {
                e.map(x.len() as u64)?;

                for (k, v) in x.iter() {
                    k.encode(e, ctx)?;
                    v.encode(e, ctx)?;
                }
            }
            NonEmptyKeyValuePairs::Indef(x) => {
                e.begin_map()?;

                for (k, v) in x.iter() {
                    k.encode(e, ctx)?;
                    v.encode(e, ctx)?;
                }

                e.end()?;
            }
        }

        Ok(())
    }
}
