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

use std::{borrow::Cow, cmp::Ordering, collections::BTreeMap, ops::Deref};

use crate::{AsIndex, Redeemer};

/// A type that provides Ord and PartialOrd instance on redeemers, to allow storing them in binary
/// trees in a controlled order (that matches Haskell's implementation).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrderedRedeemer<'a>(Cow<'a, Redeemer>);

impl Ord for OrderedRedeemer<'_> {
    fn cmp(&self, other: &Self) -> Ordering {
        match self.tag.as_index().cmp(&other.tag.as_index()) {
            by_tag @ Ordering::Less | by_tag @ Ordering::Greater => by_tag,
            Ordering::Equal => self.index.cmp(&other.index),
        }
    }
}

impl PartialOrd for OrderedRedeemer<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Deref for OrderedRedeemer<'_> {
    type Target = Redeemer;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'a> From<Cow<'a, Redeemer>> for OrderedRedeemer<'a> {
    fn from(value: Cow<'a, Redeemer>) -> Self {
        Self(value)
    }
}

impl From<Redeemer> for OrderedRedeemer<'static> {
    fn from(value: Redeemer) -> Self {
        Self(Cow::Owned(value))
    }
}

impl<'a> From<&'a Redeemer> for OrderedRedeemer<'a> {
    fn from(value: &'a Redeemer) -> Self {
        Self(Cow::Borrowed(value))
    }
}

pub struct UpsertMap<K: Ord, V>(BTreeMap<K, V>);

impl<K: Ord, V> UpsertMap<K, V> {
    pub fn new() -> Self {
        Self(BTreeMap::new())
    }

    pub fn upsert(&mut self, key: K, value: V) {
        self.0.remove(&key);
        self.0.insert(key, value);
    }

    pub fn from_iter_last_in(iter: impl IntoIterator<Item = (K, V)>) -> Self {
        let mut map = Self::new();
        for (k, v) in iter {
            map.upsert(k, v);
        }
        map
    }

    pub fn into_inner(self) -> BTreeMap<K, V> {
        self.0
    }
}

impl<K: Ord, V> Default for UpsertMap<K, V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: Ord, V> Deref for UpsertMap<K, V> {
    type Target = BTreeMap<K, V>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::{ExUnits, PlutusData, ScriptPurpose};

    fn make_redeemer(tag: ScriptPurpose, index: u32, mem: u64, steps: u64) -> Redeemer {
        Redeemer {
            tag,
            index,
            data: PlutusData::BigInt(pallas_primitives::BigInt::Int(pallas_codec::utils::Int::from(0))),
            ex_units: ExUnits { mem, steps },
        }
    }

    #[test]
    fn eq_consistent_with_ord_same_tag_index_different_ex_units() {
        let a = OrderedRedeemer::from(make_redeemer(ScriptPurpose::Spend, 0, 100, 200));
        let b = OrderedRedeemer::from(make_redeemer(ScriptPurpose::Spend, 0, 999, 888));
        assert_eq!(a.cmp(&b), Ordering::Equal);
        assert_eq!(a, b);
    }

    #[test]
    fn eq_consistent_with_ord_different_tag() {
        let a = OrderedRedeemer::from(make_redeemer(ScriptPurpose::Spend, 0, 100, 200));
        let b = OrderedRedeemer::from(make_redeemer(ScriptPurpose::Mint, 0, 100, 200));
        assert_ne!(a.cmp(&b), Ordering::Equal);
        assert_ne!(a, b);
    }

    #[test]
    fn eq_consistent_with_ord_different_index() {
        let a = OrderedRedeemer::from(make_redeemer(ScriptPurpose::Spend, 0, 100, 200));
        let b = OrderedRedeemer::from(make_redeemer(ScriptPurpose::Spend, 1, 100, 200));
        assert_ne!(a.cmp(&b), Ordering::Equal);
        assert_ne!(a, b);
    }

    #[test]
    fn btreemap_insert_keeps_first_key() {
        let r1 = OrderedRedeemer::from(make_redeemer(ScriptPurpose::Spend, 0, 100, 200));
        let r2 = OrderedRedeemer::from(make_redeemer(ScriptPurpose::Spend, 0, 999, 888));

        let mut map = BTreeMap::new();
        map.insert(r1, "first");
        map.insert(r2, "second");

        assert_eq!(map.len(), 1);
        let (key, value) = map.iter().next().unwrap();
        assert_eq!(*value, "second");
        assert_eq!(key.ex_units, ExUnits { mem: 100, steps: 200 });
    }

    #[test]
    fn upsert_map_replaces_key_and_value() {
        let r1 = OrderedRedeemer::from(make_redeemer(ScriptPurpose::Spend, 0, 100, 200));
        let r2 = OrderedRedeemer::from(make_redeemer(ScriptPurpose::Spend, 0, 999, 888));

        let mut map = UpsertMap::new();
        map.upsert(r1, "first");
        map.upsert(r2, "second");

        assert_eq!(map.len(), 1);
        let (key, value) = map.iter().next().unwrap();
        assert_eq!(*value, "second");
        assert_eq!(key.ex_units, ExUnits { mem: 999, steps: 888 });
    }
}
