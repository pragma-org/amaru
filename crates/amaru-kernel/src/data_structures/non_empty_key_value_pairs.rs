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

use crate::{KeyValuePairs, cbor, data_structures::key_value_pairs::has_duplicate};
use std::{collections::BTreeMap, ops::Deref};

/// A key-value map with at least one key:value element, and no duplicate keys.
///
/// Note: we use an underlying `Vec` to:
///
/// - keep the order of elements unchanged from original values;
/// - lower requirements on `K` & `V`;
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct NonEmptyKeyValuePairs<K: Eq, V>(Vec<(K, V)>);

impl<K: Eq + Clone, V: Clone> NonEmptyKeyValuePairs<K, V> {
    // TODO: Temporary conversion method to Pallas primitive. Remove when no longer needed.
    pub fn as_pallas(self) -> pallas_primitives::NonEmptyKeyValuePairs<K, V> {
        pallas_primitives::NonEmptyKeyValuePairs::Def(self.0)
    }
}

impl<K: Eq, V> From<NonEmptyKeyValuePairs<K, V>> for Vec<(K, V)> {
    fn from(pairs: NonEmptyKeyValuePairs<K, V>) -> Self {
        pairs.0
    }
}

impl<K: Ord, V> From<NonEmptyKeyValuePairs<K, V>> for BTreeMap<K, V> {
    fn from(set: NonEmptyKeyValuePairs<K, V>) -> Self {
        BTreeMap::from_iter(Vec::from(set))
    }
}

impl<K: Eq, V> TryFrom<Vec<(K, V)>> for NonEmptyKeyValuePairs<K, V> {
    type Error = IntoNonEmptyKeyValuePairsError;

    fn try_from(vec: Vec<(K, V)>) -> Result<Self, Self::Error> {
        if vec.is_empty() {
            return Err(Self::Error::Empty);
        }

        if has_duplicate(vec.as_slice()) {
            return Err(Self::Error::HasDuplicate);
        }

        Ok(Self(vec))
    }
}

impl<K: Eq, V> Deref for NonEmptyKeyValuePairs<K, V> {
    type Target = [(K, V)];

    fn deref(&self) -> &Self::Target {
        self.0.as_slice()
    }
}

impl<K: Eq, V> AsRef<[(K, V)]> for NonEmptyKeyValuePairs<K, V> {
    fn as_ref(&self) -> &[(K, V)] {
        self.0.deref()
    }
}

impl<K: Eq, V> IntoIterator for NonEmptyKeyValuePairs<K, V> {
    type Item = (K, V);
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<C, K, V> cbor::encode::Encode<C> for NonEmptyKeyValuePairs<K, V>
where
    K: cbor::Encode<C> + Eq,
    V: cbor::Encode<C>,
{
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.map(self.len() as u64)?;
        for (k, v) in self.iter() {
            k.encode(e, ctx)?;
            v.encode(e, ctx)?;
        }
        Ok(())
    }
}

impl<'b, C, K, V> cbor::decode::Decode<'b, C> for NonEmptyKeyValuePairs<K, V>
where
    K: for<'k> cbor::Decode<'k, ()> + Eq,
    V: for<'v> cbor::Decode<'v, C>,
{
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        let position = d.position();

        let kvs: KeyValuePairs<K, V> = d.decode_with(ctx)?;

        let kvs: Vec<(K, V)> = Vec::from(kvs);

        // Ensure non-empty.
        if kvs.is_empty() {
            return Err(
                cbor::decode::Error::message(IntoNonEmptyKeyValuePairsError::Empty).at(position),
            );
        }

        Ok(Self(kvs))
    }
}

// ----------------------------------------------------------------------------
// IntoNonEmptyKeyValuePairsError
// ----------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum IntoNonEmptyKeyValuePairsError {
    #[error("empty map when expecting at least one key/value pair")]
    Empty,
    #[error("found duplicate keys when converting collection to a key/value map")]
    HasDuplicate,
}

#[cfg(test)]
mod tests {
    use super::NonEmptyKeyValuePairs;
    use crate::{from_cbor_no_leftovers, to_cbor};
    use proptest::{collection, prelude::*};
    use std::collections::BTreeMap;
    use test_case::test_case;

    proptest! {
        #[test]
        fn roundtrip_encode_decode(elems in collection::vec((any::<u8>(), any::<u8>()), 1..100)) {
            let set: Vec<(u8, u8)> = BTreeMap::from_iter(elems).into_iter().collect();
            let non_empty_map: NonEmptyKeyValuePairs<u8, u8> = NonEmptyKeyValuePairs::try_from(set).unwrap();
            assert_eq!(
                from_cbor_no_leftovers::<NonEmptyKeyValuePairs<u8, u8>>(to_cbor(&non_empty_map).as_slice()).unwrap(),
                non_empty_map,
            )
        }
    }

    #[test_case("A1016161", &[(1, "a")], true; "singleton")]
    #[test_case("BF016161026162036161FF", &[(1, "a"), (2, "b"), (3, "a")], false; "indef map")]
    #[test_case("A3046162016163026161", &[(4, "b"), (1, "c"), (2, "a")], true; "def map")]
    fn from_cbor_success(s: &str, expected: &[(u8, &str)], expected_roundtrip: bool) {
        let original_bytes = hex::decode(s).unwrap();
        match from_cbor_no_leftovers::<NonEmptyKeyValuePairs<u8, String>>(original_bytes.as_slice())
        {
            Ok(set) => {
                assert_eq!(
                    set.iter()
                        .map(|(k, v)| (*k, v.as_str()))
                        .collect::<Vec<_>>()
                        .as_slice(),
                    expected
                );
                let bytes = to_cbor(&set);
                assert_eq!(
                    bytes == original_bytes,
                    expected_roundtrip,
                    "bytes={}, original_bytes={s}, expected_roundtrip={expected_roundtrip}",
                    hex::encode(&bytes),
                );
            }
            Err(err) => panic!("{err}"),
        }
    }

    #[test_case("A0"; "empty map")]
    #[test_case("A3010002000100"; "with duplicates")]
    #[test_case("D901028101"; "not an map")]
    #[test_case("A2040001000200"; "leftovers")]
    fn from_cbor_failures(s: &str) {
        assert!(matches!(
            from_cbor_no_leftovers::<NonEmptyKeyValuePairs<u8, u8>>(
                hex::decode(s).unwrap().as_slice()
            ),
            Err(..),
        ));
    }
}
