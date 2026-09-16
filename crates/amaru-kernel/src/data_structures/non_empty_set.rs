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

use std::{collections::BTreeSet, ops::Deref};

use crate::cbor;

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

/// What to do with two entries that `same` says are the same.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Duplicates {
    /// Fail with `IntoNonEmptySetError::HasDuplicate`.
    Reject,
    /// Keep the last of the same entries, as `Data.Set.insert` and `Data.Map.fromList` do in the
    /// Haskell ledger.
    KeepLast,
}

impl<T: Eq> NonEmptySet<T> {
    /// Build a set from a vector, with a caller-chosen sameness test and duplicate policy.
    ///
    /// The sameness test is not always `T::eq`: the Haskell ledger keys some witness collections
    /// on a hash whose preimage is only a part of the entry.
    pub fn from_vec_by(
        vec: Vec<T>,
        same: impl Fn(&T, &T) -> bool,
        duplicates: Duplicates,
    ) -> Result<Self, IntoNonEmptySetError> {
        let vec = match duplicates {
            Duplicates::Reject if has_duplicate_by(vec.as_slice(), &same) => Err(IntoNonEmptySetError::HasDuplicate),
            Duplicates::Reject => Ok(vec),
            Duplicates::KeepLast => Ok(keep_last_by(vec, &same)),
        }?;

        if vec.is_empty() { Err(IntoNonEmptySetError::Empty) } else { Ok(Self(vec)) }
    }

    /// Decode a set, with a caller-chosen sameness test and duplicate policy. The set tag 258 is
    /// optional; definite and indefinite arrays both decode.
    pub fn decode_by<'b, C>(
        d: &mut cbor::Decoder<'b>,
        ctx: &mut C,
        same: impl Fn(&T, &T) -> bool,
        duplicates: Duplicates,
    ) -> Result<Self, cbor::decode::Error>
    where
        T: cbor::Decode<'b, C>,
    {
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

        Self::from_vec_by(vec, same, duplicates).map_err(|e| cbor::decode::Error::message(e).at(position))
    }
}

impl<T: Eq> TryFrom<Vec<T>> for NonEmptySet<T> {
    type Error = IntoNonEmptySetError;

    fn try_from(vec: Vec<T>) -> Result<Self, Self::Error> {
        Self::from_vec_by(vec, T::eq, Duplicates::Reject)
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
        Self::decode_by(d, ctx, T::eq, Duplicates::Reject)
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

/// Check whether a slice contains duplicates under a caller-chosen sameness test, minimizing
/// allocation. The check compares every pair once, so it is quadratic in the slice length.
///
/// We do not use HashSet or BTreeSet for mainly two reasons:
///
/// 1. They introduce additional requirements on `T` (Hash in one case, and Ord on the other).
/// 2. We want to preserve the underlying order when possible;
pub(crate) fn has_duplicate_by<T>(xs: &[T], same: impl Fn(&T, &T) -> bool) -> bool {
    xs.iter().enumerate().any(|(i, x)| xs.iter().skip(i + 1).any(|y| same(x, y)))
}

/// Drop every entry that a later entry is the same as, so the LAST of two same entries survives.
/// This is what `Data.Set.insert` and `Data.Map.fromList` do in the Haskell ledger.
fn keep_last_by<T>(xs: Vec<T>, same: impl Fn(&T, &T) -> bool) -> Vec<T> {
    let kept: Vec<bool> = xs.iter().enumerate().map(|(i, x)| !xs.iter().skip(i + 1).any(|y| same(x, y))).collect();

    xs.into_iter().zip(kept).filter_map(|(x, keep)| keep.then_some(x)).collect()
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeSet, ops::Deref};

    use proptest::{collection, prelude::*};
    use test_case::test_case;

    use super::{Duplicates, IntoNonEmptySetError, NonEmptySet, has_duplicate_by};
    use crate::{cbor, from_cbor_no_leftovers, to_cbor};

    #[test]
    fn has_duplicate_empty() {
        let slice: &[u8] = &[];
        assert!(!has_duplicate_by(slice, PartialEq::eq))
    }

    #[test_case(vec![1, 2, 1], Duplicates::KeepLast => Ok(vec![2, 1]); "keep last, one duplicate")]
    #[test_case(vec![3, 1, 4, 1, 5], Duplicates::KeepLast => Ok(vec![3, 4, 1, 5]); "keep last, interleaved")]
    #[test_case(vec![1, 1, 1], Duplicates::KeepLast => Ok(vec![1]); "keep last, all the same")]
    #[test_case(vec![], Duplicates::KeepLast => Err("empty".to_string()); "keep last, empty")]
    #[test_case(vec![1, 2, 1], Duplicates::Reject => Err("duplicate".to_string()); "reject, one duplicate")]
    #[test_case(vec![1, 2, 3], Duplicates::Reject => Ok(vec![1, 2, 3]); "reject, no duplicate")]
    fn from_vec_by(vec: Vec<u8>, duplicates: Duplicates) -> Result<Vec<u8>, String> {
        NonEmptySet::from_vec_by(vec, u8::eq, duplicates).map(Vec::from).map_err(|e| match e {
            IntoNonEmptySetError::Empty => "empty".to_string(),
            IntoNonEmptySetError::HasDuplicate => "duplicate".to_string(),
        })
    }

    #[test_case("D9010283010201", Duplicates::KeepLast => Ok(vec![2, 1]); "tagged array, keep last")]
    #[test_case("D9010283010201", Duplicates::Reject => Err(()); "tagged array, reject")]
    fn decode_by(s: &str, duplicates: Duplicates) -> Result<Vec<u8>, ()> {
        let bytes = hex::decode(s).unwrap();
        let mut d = cbor::Decoder::new(bytes.as_slice());
        NonEmptySet::<u8>::decode_by(&mut d, &mut (), u8::eq, duplicates).map(Vec::from).map_err(|_| ())
    }

    #[test_case(&[1], false)]
    #[test_case(&[1, 1], true)]
    #[test_case(&[1, 2, 3, 4, 5], false)]
    #[test_case(&[1, 2, 2, 4, 5], true)]
    #[test_case(&[1, 2, 3, 4, 4], true)]
    #[test_case(&[3, 1, 4, 2, 3], true)]
    fn has_duplicate_non_empty(slice: &[u8], result: bool) {
        assert!(has_duplicate_by(slice, PartialEq::eq) == result, "{slice:?}");
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
        assert!(matches!(from_cbor_no_leftovers::<NonEmptySet<u8>>(hex::decode(s).unwrap().as_slice()), Err(..),));
    }
}
