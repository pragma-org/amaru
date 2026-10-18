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

/// A read-only set of unique values, held in ascending order in a `Vec`.
///
/// The CBOR representation is an array, optionally tagged with 258 below protocol version 12. Its
/// order carries no information and is discarded on decoding. A repeated element is rejected.
///
// NOTE: we use a 'Vec' internally rather than 'BTreeSet'
//
//  A `BTreeSet` allocates in nodes of ~400 bytes, which is a poor trade for a small set living in a
//  structure that is retained and reproduced many times — a pool's owners, most often a single one.
//  A sorted `Vec` keeps those elements contiguous, and searching a small one costs little.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "Vec<T>")]
pub struct VecSet<T: Ord>(Vec<T>);

impl<T: Ord> Default for VecSet<T> {
    fn default() -> Self {
        Self(Vec::new())
    }
}

impl<T: Ord> VecSet<T> {
    /// Whether the set holds `element`, found by bisecting the elements.
    pub fn contains(&self, element: &T) -> bool {
        self.0.binary_search(element).is_ok()
    }
}

impl<T: Ord> TryFrom<Vec<T>> for VecSet<T> {
    type Error = IntoVecSetError;

    fn try_from(mut vec: Vec<T>) -> Result<Self, Self::Error> {
        vec.sort_unstable();

        if vec.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(Self::Error::HasDuplicate);
        }

        Ok(Self(vec))
    }
}

#[cfg(any(test, feature = "test-utils"))]
impl<T: Ord> From<BTreeSet<T>> for VecSet<T> {
    fn from(value: BTreeSet<T>) -> Self {
        VecSet(value.into_iter().collect())
    }
}

/// Collect values that are known to be unique already, such as the contents of another set. A
/// repeat is collapsed here rather than rejected; [`TryFrom<Vec<T>>`] is the checking way in.
impl<T: Ord> FromIterator<T> for VecSet<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        let mut elements = iter.into_iter().collect::<Vec<_>>();
        elements.sort_unstable();
        elements.dedup();
        Self(elements)
    }
}

impl<T: Ord> From<VecSet<T>> for Vec<T> {
    fn from(set: VecSet<T>) -> Self {
        set.0
    }
}

impl<T: Ord> IntoIterator for VecSet<T> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

/// Hand out the elements in ascending order. Borrowing iteration goes through the `Deref` to a
/// slice below.
impl<'a, T: Ord> IntoIterator for &'a VecSet<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl<T: Ord> Deref for VecSet<T> {
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        self.0.as_slice()
    }
}

impl<T: Ord> AsRef<[T]> for VecSet<T> {
    fn as_ref(&self) -> &[T] {
        self.0.as_slice()
    }
}

impl<C, T> cbor::Encode<C> for VecSet<T>
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

impl<'b, C, T> cbor::Decode<'b, C> for VecSet<T>
where
    T: Ord + cbor::Decode<'b, C>,
{
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        // The set tag is optional below protocol version 12, and becomes mandatory at it.
        if d.datatype()? == cbor::Type::Tag {
            let expected_tag = cbor::TAG_SET_258;
            let found_tag = d.tag()?;
            if found_tag != expected_tag {
                return Err(cbor::decode::Error::tag_mismatch(expected_tag));
            }
        }

        let position = d.position();
        let elements: Vec<T> = d.decode_with(ctx)?;

        Self::try_from(elements).map_err(|e| cbor::decode::Error::message(e).at(position))
    }
}

// ----------------------------------------------------------------------------
// IntoVecSetError
// ----------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum IntoVecSetError {
    #[error("found duplicate elements when converting collection to a set")]
    HasDuplicate,
}

#[cfg(test)]
mod tests {
    use test_case::test_case;

    use super::VecSet;
    use crate::{from_cbor_no_leftovers, to_cbor};

    #[test_case("d9010283010203", &[1, 2, 3]; "a tagged set")]
    #[test_case("83010203", &[1, 2, 3]; "an untagged array")]
    #[test_case("9f010203ff", &[1, 2, 3]; "an indefinite-length array")]
    #[test_case("83030102", &[1, 2, 3]; "elements out of order are ordered")]
    #[test_case("80", &[]; "the empty set")]
    fn decodes_into_ascending_order(input: &str, expected: &[u8]) {
        let set: VecSet<u8> = from_cbor_no_leftovers(&hex::decode(input).unwrap()).unwrap();

        assert_eq!(set.as_ref(), expected);
    }

    /// Whatever shape it arrived in, a set is written as a tagged array in ascending order, which is
    /// how the ledger writes the fields it holds in a `Set`.
    #[test_case("83030102"; "out of order")]
    #[test_case("9f030102ff"; "indefinite-length")]
    #[test_case("d9010283010203"; "already tagged and ordered")]
    fn always_re_encodes_as_an_ascending_tagged_set(input: &str) {
        let set: VecSet<u8> = from_cbor_no_leftovers(&hex::decode(input).unwrap()).unwrap();

        assert_eq!(hex::encode(to_cbor(&set)), "d9010283010203");
    }

    #[test_case("830101"; "a repeated element")]
    #[test_case("d901028401010203"; "a repeated element in a tagged set")]
    #[test_case("d9010383010203"; "the wrong tag")]
    fn rejects(input: &str) {
        assert!(from_cbor_no_leftovers::<VecSet<u8>>(&hex::decode(input).unwrap()).is_err());
    }

    #[test]
    fn contains_finds_every_element_and_nothing_else() {
        let set = VecSet::try_from(vec![3u8, 1, 2]).unwrap();

        assert!(set.contains(&1) && set.contains(&2) && set.contains(&3));
        assert!(!set.contains(&0) && !set.contains(&4));
    }
}
