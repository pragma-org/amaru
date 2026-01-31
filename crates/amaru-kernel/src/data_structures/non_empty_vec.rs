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
use std::{collections::BTreeSet, fmt::Debug, ops::Deref};

/// A read-only non-empty vector: an ordered set of values with at least one element.
#[derive(Debug, PartialEq, Eq, Clone, PartialOrd, serde::Serialize, serde::Deserialize)]
pub struct NonEmptyVec<T: Eq>(Vec<T>);

impl<T: Eq> From<NonEmptyVec<T>> for Vec<T> {
    fn from(elems: NonEmptyVec<T>) -> Self {
        elems.0
    }
}

impl<T: Eq + Ord> From<NonEmptyVec<T>> for BTreeSet<T> {
    fn from(elems: NonEmptyVec<T>) -> Self {
        BTreeSet::from_iter(Vec::from(elems))
    }
}

impl<T: Eq + Debug> TryFrom<Vec<T>> for NonEmptyVec<T> {
    type Error = IntoNonEmptyVecError;

    fn try_from(vec: Vec<T>) -> Result<Self, Self::Error> {
        if vec.is_empty() {
            return Err(Self::Error::Empty);
        }

        Ok(Self(vec))
    }
}

impl<T: Eq> From<NonEmptyVec<KeepRaw<'_, T>>> for NonEmptyVec<T> {
    fn from(value: NonEmptyVec<KeepRaw<'_, T>>) -> Self {
        let inner = value.0.into_iter().map(|x| x.unwrap()).collect();
        Self(inner)
    }
}

impl<T: Eq> AsRef<[T]> for NonEmptyVec<T> {
    fn as_ref(&self) -> &[T] {
        self.0.deref()
    }
}

impl<T: Eq> Deref for NonEmptyVec<T> {
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        self.0.as_slice()
    }
}

impl<C, T> cbor::encode::Encode<C> for NonEmptyVec<T>
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

impl<'b, C, T> cbor::Decode<'b, C> for NonEmptyVec<T>
where
    T: Eq + Debug + cbor::Decode<'b, C>,
{
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        // NOTE: Set tag for non-empty vectors
        //
        // This looks weird, doesn't it? This is because non_empty_vec were meant to be non-empty
        // ordered set; but ... bugs happened. See the note about verification key and bootstrap
        // witnesses on WitnessSet
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
// IntoNonEmptyVecError
// ----------------------------------------------------------------------------

/// Errors that may occur when constructing a NonEmptyVec.
#[derive(Debug, thiserror::Error)]
pub enum IntoNonEmptyVecError {
    #[error("empty set when expecting at least one element")]
    Empty,
}

#[cfg(test)]
mod tests {
    use super::NonEmptyVec;
    use crate::{from_cbor_no_leftovers, to_cbor};
    use proptest::{collection, prelude::*};
    use std::{collections::BTreeSet, ops::Deref};
    use test_case::test_case;

    proptest! {
        #[test]
        fn roundtrip_encode_decode(elems in collection::vec(any::<u8>(), 1..100)) {
            let set: Vec<u8> = BTreeSet::from_iter(elems).into_iter().collect();
            let non_empty_set: NonEmptyVec<u8> = NonEmptyVec::try_from(set).unwrap();
            assert_eq!(
                from_cbor_no_leftovers::<NonEmptyVec<u8>>(to_cbor(&non_empty_set).as_slice()).unwrap(),
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
    #[test_case("83010201", &[1, 2, 1], false; "with duplicates")]
    #[test_case("D901028401010203", &[1,1,2,3], true; "tagged with duplicates")]
    fn from_cbor_success(s: &str, expected: &[u8], expected_roundtrip: bool) {
        let original_bytes = hex::decode(s).unwrap();
        match from_cbor_no_leftovers::<NonEmptyVec<u8>>(original_bytes.as_slice()) {
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
    #[test_case("80"; "empty set")]
    #[test_case("D90102A10102"; "not an array")]
    #[test_case("D9010282010203"; "leftovers")]
    #[test_case("D81B8101"; "unknown tag")]
    fn from_cbor_failures(s: &str) {
        assert!(matches!(
            from_cbor_no_leftovers::<NonEmptyVec<u8>>(hex::decode(s).unwrap().as_slice()),
            Err(..),
        ));
    }
}
