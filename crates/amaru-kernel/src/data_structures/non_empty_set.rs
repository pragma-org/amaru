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

use crate::{KeepRaw, cbor};
use std::{collections::BTreeSet, ops::Deref};

/// A read-only non-empty set: unique set of values with at least one element.
///
/// NOTE: use of 'Vec' on 'NonEmptySet'
///
///   We use an underlying `Vec` to
///   - keep the order of elements unchanged from original values;
///   - lower requirements on `T`.
#[derive(Debug, PartialEq, Eq, Clone, PartialOrd, serde::Serialize, serde::Deserialize)]
pub struct NonEmptySet<T: Eq>(Vec<T>);

impl<T: Eq> From<NonEmptySet<T>> for Vec<T> {
    fn from(set: NonEmptySet<T>) -> Self {
        set.0
    }
}

impl<T: Eq + Ord> From<NonEmptySet<T>> for BTreeSet<T> {
    fn from(set: NonEmptySet<T>) -> Self {
        BTreeSet::from_iter(Vec::from(set))
    }
}

impl<T: Eq> TryFrom<Vec<T>> for NonEmptySet<T> {
    type Error = IntoNonEmptySetError;

    fn try_from(vec: Vec<T>) -> Result<Self, Self::Error> {
        if vec.is_empty() {
            return Err(Self::Error::Empty);
        }

        if has_duplicate(vec.as_slice()) {
            return Err(Self::Error::HasDuplicate);
        }

        Ok(Self(vec))
    }
}

impl<T: Eq> From<NonEmptySet<KeepRaw<'_, T>>> for NonEmptySet<T> {
    fn from(value: NonEmptySet<KeepRaw<'_, T>>) -> Self {
        let inner = value.0.into_iter().map(|x| x.unwrap()).collect();
        Self(inner)
    }
}

impl<T: Eq> AsRef<[T]> for NonEmptySet<T> {
    fn as_ref(&self) -> &[T] {
        self.0.deref()
    }
}

impl<T: Eq> Deref for NonEmptySet<T> {
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        self.0.as_slice()
    }
}

impl<C, T> cbor::encode::Encode<C> for NonEmptySet<T>
where
    T: Eq + cbor::Encode<C>,
{
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.tag(cbor::TAG_SET_258)?;
        e.encode_with(self.deref(), ctx)?;
        Ok(())
    }
}

impl<'b, C, T> cbor::Decode<'b, C> for NonEmptySet<T>
where
    T: Eq + cbor::Decode<'b, C>,
{
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        // optional set tag (this will be required in era following Conway)
        if d.datatype()? == cbor::Type::Tag {
            let expected_tag = cbor::TAG_SET_258;
            let found_tag = d.tag()?;
            if found_tag != expected_tag {
                return Err(cbor::decode::Error::tag_mismatch(expected_tag));
            }
        }

        let position = d.position();

        let vec: Vec<T> = d.decode_with(ctx)?;

        Self::try_from(vec).map_err(|e| cbor::decode::Error::message(e).at(position))
    }
}

// ----------------------------------------------------------------------------
// IntoNonEmptySetError
// ----------------------------------------------------------------------------

/// Errors that may occur when constructing a NonEmptySet.
#[derive(Debug, thiserror::Error)]
pub enum IntoNonEmptySetError {
    #[error("empty set when expecting at least one element")]
    Empty,
    #[error("found duplicate elements when converting collection to a set")]
    HasDuplicate,
}

// ----------------------------------------------------------------------------
// Internals
// ----------------------------------------------------------------------------

/// Check whether a slice contains duplicate relying only on the `Eq` instance and minimizing
/// allocation. The check is still in O(n*log(n)).
///
/// We do not use HashSet or BTreeSet for mainly two reasons:
///
/// 1. They introduce additional requirements on `T` (Hash in one case, and Ord on the other).
/// 2. We want to preserve the underlying order when possible;
///
/// Pre-condition: the slice is NOT empty.
pub(crate) fn has_duplicate<T: Eq>(xs: &[T]) -> bool {
    let last = xs.len() - 1;

    for i in 0..last {
        let x1 = &xs[i];
        for x2 in xs.iter().take(last + 1).skip(i + 1) {
            if x1 == x2 {
                return true;
            }
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::{NonEmptySet, has_duplicate};
    use crate::{from_cbor_no_leftovers, to_cbor};
    use proptest::{collection, prelude::*};
    use std::{collections::BTreeSet, ops::Deref};
    use test_case::test_case;

    #[test]
    fn has_duplicate_empty() {
        assert!(matches!(
            std::panic::catch_unwind(|| {
                let slice: &[u8] = &[];
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
        assert!(has_duplicate(slice) == result, "{slice:?}");
    }

    proptest! {
        #[test]
        fn roundtrip_encode_decode(elems in collection::vec(any::<u8>(), 1..100)) {
            let set: Vec<u8> = BTreeSet::from_iter(elems).into_iter().collect();
            let non_empty_set: NonEmptySet<u8> = NonEmptySet::try_from(set).unwrap();
            assert_eq!(
                from_cbor_no_leftovers::<NonEmptySet<u8>>(to_cbor(&non_empty_set).as_slice()).unwrap(),
                non_empty_set,
            )
        }
    }

    #[test_case("D901028101", &[1], true; "tagged singleton")]
    #[test_case("8101", &[1], false; "singleton")]
    #[test_case("D901029F010203FF", &[1,2,3], false; "tagged indef array")]
    #[test_case("9F010203FF", &[1,2,3], false; "indef array")]
    #[test_case("D9010283040102", &[4, 1, 2], true; "tagged def array")]
    #[test_case("83040102", &[4, 1, 2], false; "def array")]
    fn from_cbor_success(s: &str, expected: &[u8], expected_roundtrip: bool) {
        let original_bytes = hex::decode(s).unwrap();
        match from_cbor_no_leftovers::<NonEmptySet<u8>>(original_bytes.as_slice()) {
            Ok(set) => {
                assert_eq!(set.deref(), expected);
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

    #[test_case("D9010280"; "empty tagged set")]
    #[test_case("D901028401010203"; "tagged with duplicates")]
    #[test_case("80"; "empty set")]
    #[test_case("83010201"; "with duplicates")]
    #[test_case("D90102A10102"; "not an array")]
    #[test_case("D9010282010203"; "leftovers")]
    #[test_case("D81B8101"; "unknown tag")]
    fn from_cbor_failures(s: &str) {
        assert!(matches!(
            from_cbor_no_leftovers::<NonEmptySet<u8>>(hex::decode(s).unwrap().as_slice()),
            Err(..),
        ));
    }
}
