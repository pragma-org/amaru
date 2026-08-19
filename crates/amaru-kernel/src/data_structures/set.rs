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

use std::{
    collections::{BTreeSet, btree_set},
    ops::Deref,
};

use crate::cbor;

/// A read-only set of unique values, held in ascending order.
///
/// The CBOR representation is an array, optionally tagged with 258. Its order carries no
/// information and is discarded on decoding; duplicate elements are rejected rather than
/// collapsed.
// NOTE: use of 'BTreeSet' in 'Set'
//
//  Unlike [`crate::NonEmptySet`], which keeps its elements in a `Vec`, this type is unsuitable
//  for values retained beyond validation: a `BTreeSet` allocates in nodes of ~400 bytes, which
//  is a poor trade for a small set held in a long-lived structure
#[derive(Debug, PartialEq, Eq, Clone, serde::Serialize, serde::Deserialize)]
pub struct Set<T: Ord>(BTreeSet<T>);

impl<T: Ord> Default for Set<T> {
    fn default() -> Self {
        Self(BTreeSet::new())
    }
}

impl<T: Ord> TryFrom<Vec<T>> for Set<T> {
    type Error = IntoSetError;

    fn try_from(vec: Vec<T>) -> Result<Self, Self::Error> {
        let length = vec.len();

        let set = BTreeSet::from_iter(vec);

        if set.len() != length {
            return Err(Self::Error::HasDuplicate);
        }

        Ok(Self(set))
    }
}

impl<T: Ord> FromIterator<T> for Set<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        Self(BTreeSet::from_iter(iter))
    }
}

impl<T: Ord> IntoIterator for Set<T> {
    type Item = T;
    type IntoIter = btree_set::IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a, T: Ord> IntoIterator for &'a Set<T> {
    type Item = &'a T;
    type IntoIter = btree_set::Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl<T: Ord> Deref for Set<T> {
    type Target = BTreeSet<T>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<C, T> cbor::encode::Encode<C> for Set<T>
where
    T: Ord + cbor::Encode<C>,
{
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.tag(cbor::TAG_SET_258)?;
        e.encode_with(&self.0, ctx)?;
        Ok(())
    }
}

impl<'b, C, T> cbor::Decode<'b, C> for Set<T>
where
    T: Ord + cbor::Decode<'b, C>,
{
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
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

#[derive(Debug, thiserror::Error)]
pub enum IntoSetError {
    #[error("found duplicate elements when converting collection to a set")]
    HasDuplicate,
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeSet, ops::Deref};

    use proptest::{collection, prelude::*};
    use test_case::test_case;

    use super::Set;
    use crate::{from_cbor_no_leftovers, to_cbor};

    proptest! {
        #[test]
        fn roundtrip_encode_decode(elems in collection::vec(any::<u8>(), 0..100)) {
            let set = Set::from_iter(elems);
            assert_eq!(from_cbor_no_leftovers::<Set<u8>>(to_cbor(&set).as_slice()).unwrap(), set)
        }
    }

    #[test_case("D9010280", &[]; "empty tagged set")]
    #[test_case("80", &[]; "empty set")]
    #[test_case("D901028101", &[1]; "tagged singleton")]
    #[test_case("8101", &[1]; "singleton")]
    #[test_case("D901029F010203FF", &[1,2,3]; "tagged indef array")]
    #[test_case("9F010203FF", &[1,2,3]; "indef array")]
    #[test_case("D9010283040102", &[1, 2, 4]; "tagged out of order")]
    #[test_case("83040102", &[1, 2, 4]; "out of order")]
    fn from_cbor_success(s: &str, expected: &[u8]) {
        match from_cbor_no_leftovers::<Set<u8>>(hex::decode(s).unwrap().as_slice()) {
            Ok(set) => assert_eq!(set.deref(), &BTreeSet::from_iter(expected.iter().copied())),
            Err(err) => panic!("{err}"),
        }
    }

    #[test_case("D901028401010203"; "tagged with duplicates")]
    #[test_case("83010201"; "with duplicates")]
    #[test_case("D90102A10102"; "not an array")]
    #[test_case("D9010282010203"; "leftovers")]
    #[test_case("D81B8101"; "unknown tag")]
    fn from_cbor_failures(s: &str) {
        assert!(matches!(from_cbor_no_leftovers::<Set<u8>>(hex::decode(s).unwrap().as_slice()), Err(..)));
    }
}
