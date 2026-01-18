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
    Deref,
    cbor::{Decode, Encode, decode::Error},
};
use amaru_minicbor_extra::heterogeneous_map;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A key-value map with at least one key:value element, and no duplicate keys.
///
/// Note: we use an underlying `Vec` to:
///
/// - keep the order of elements unchanged from original values;
/// - lower requirements on `K` & `V`;
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
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

impl<C, K, V> minicbor::encode::Encode<C> for NonEmptyKeyValuePairs<K, V>
where
    K: Encode<C> + Eq,
    V: Encode<C>,
{
    fn encode<W: minicbor::encode::Write>(
        &self,
        e: &mut minicbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), minicbor::encode::Error<W::Error>> {
        e.map(self.len() as u64)?;
        for (k, v) in self.iter() {
            k.encode(e, ctx)?;
            v.encode(e, ctx)?;
        }
        Ok(())
    }
}

impl<'b, C, K, V> minicbor::decode::Decode<'b, C> for NonEmptyKeyValuePairs<K, V>
where
    K: for<'k> Decode<'k, ()> + Eq,
    V: for<'v> Decode<'v, C>,
{
    fn decode(d: &mut minicbor::Decoder<'b>, ctx: &mut C) -> Result<Self, minicbor::decode::Error> {
        let mut inner = Vec::new();

        heterogeneous_map(
            d,
            &mut inner,
            |d| d.decode::<K>(),
            |d, st, k| {
                // Check for absence of duplicate key.
                //
                // FIXME:
                // - in protocol version < 2: enforce strict key ordering with no duplicate
                // - in protocol version >= 2 && < 9: allow (and silently ignore) duplicate keys
                for (j, _) in st.iter() {
                    if j == &k {
                        return Err(Error::message(IntoNonEmptyKeyValuePairsError::HasDuplicate));
                    }
                }
                let v = d.decode_with(ctx)?;
                st.push((k, v));
                Ok(())
            },
        )?;

        // Ensure non-empty.
        if inner.is_empty() {
            return Err(Error::message(IntoNonEmptyKeyValuePairsError::Empty));
        }

        Ok(Self(inner))
    }
}

// ----------------------------------------------------------------------------
// IntoNonEmptyKeyValuePairsError
// ----------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum IntoNonEmptyKeyValuePairsError {
    #[error("empty map when expected at least one key/value pair")]
    Empty,
    #[error("found duplicate keys when converting collection to a key/value map")]
    HasDuplicate,
}

// ----------------------------------------------------------------------------
// Internals
// ----------------------------------------------------------------------------

/// Check whether a slice contains duplicate relying only on the `Eq` instance and minimizing
/// allocation. The check is still in O(n*log(n)).
///
/// We do not use HashMap or BTreeMap for mainly two reasons:
///
/// 1. They introduce additional requirements on `K` (Hash in one case, and Ord on the other).
/// 2. We want to preserve the underlying order when possible;
///
/// Pre-condition: the slice is NOT empty.
pub(crate) fn has_duplicate<K: Eq, V>(kvs: &[(K, V)]) -> bool {
    let last = kvs.len() - 1;

    for i in 0..last {
        let (k1, _) = &kvs[i];
        for (k2, _) in kvs.iter().take(last + 1).skip(i + 1) {
            if k1 == k2 {
                return true;
            }
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::{NonEmptyKeyValuePairs, has_duplicate};
    use amaru_minicbor_extra::{from_cbor_no_leftovers, to_cbor};
    use proptest::{collection, prelude::*};
    use std::collections::BTreeMap;
    use test_case::test_case;

    #[test]
    fn has_duplicate_empty() {
        assert!(matches!(
            std::panic::catch_unwind(|| {
                let slice: &[(u8, ())] = &[];
                has_duplicate(slice)
            }),
            Err(..)
        ))
    }

    #[test_case(&[1], false)]
    #[test_case(&[1, 1], true)]
    #[test_case(&[1, 2, 3, 4, 5], false)]
    #[test_case(&[1, 2, 2, 4, 5], true)]
    #[test_case(&[1, 2, 3, 4, 4], true)]
    #[test_case(&[3, 1, 4, 2, 3], true)]
    fn has_duplicate_non_empty(slice: &[u8], result: bool) {
        let slice = slice.iter().map(|i| (i, ())).collect::<Vec<_>>();
        assert!(has_duplicate(slice.as_slice()) == result, "{slice:?}");
    }

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
