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

use crate::{VecSet, cbor};

/// A [`VecSet`] holding at least one element.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "Vec<T>")]
pub struct NonEmptyVecSet<T: Ord>(VecSet<T>);

impl<T: Ord> NonEmptyVecSet<T> {
    /// Whether the set holds `element`, found by bisecting the elements rather than walking them.
    pub fn contains(&self, element: &T) -> bool {
        self.0.contains(element)
    }
}

impl<T: Ord> TryFrom<Vec<T>> for NonEmptyVecSet<T> {
    type Error = IntoNonEmptyVecSetError;

    fn try_from(vec: Vec<T>) -> Result<Self, Self::Error> {
        if vec.is_empty() {
            return Err(Self::Error::Empty);
        }

        Ok(Self(VecSet::try_from(vec)?))
    }
}

impl<T: Ord> From<NonEmptyVecSet<T>> for Vec<T> {
    fn from(set: NonEmptyVecSet<T>) -> Self {
        set.0.into()
    }
}

impl<T: Ord> IntoIterator for NonEmptyVecSet<T> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

/// Hand out the elements in ascending order. Borrowing iteration goes through the `Deref` to a
/// slice below.
impl<'a, T: Ord> IntoIterator for &'a NonEmptyVecSet<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl<T: Ord> Deref for NonEmptyVecSet<T> {
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        self.0.deref()
    }
}

impl<T: Ord> AsRef<[T]> for NonEmptyVecSet<T> {
    fn as_ref(&self) -> &[T] {
        self.0.as_ref()
    }
}

impl<C, T> cbor::Encode<C> for NonEmptyVecSet<T>
where
    T: Ord + cbor::Encode<C>,
{
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.encode_with(&self.0, ctx)?;
        Ok(())
    }
}

impl<'b, C, T> cbor::Decode<'b, C> for NonEmptyVecSet<T>
where
    T: Ord + cbor::Decode<'b, C>,
{
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        let position = d.position();
        let elements: VecSet<T> = d.decode_with(ctx)?;

        if elements.is_empty() {
            return Err(cbor::decode::Error::message(IntoNonEmptyVecSetError::Empty).at(position));
        }

        Ok(Self(elements))
    }
}

// ----------------------------------------------------------------------------
// IntoNonEmptyVecSetError
// ----------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum IntoNonEmptyVecSetError {
    #[error("empty set when expecting at least one element")]
    Empty,
    #[error(transparent)]
    Invalid(#[from] crate::IntoVecSetError),
}

/// Define a newtype over a [`NonEmptyVecSet`], for a field the ledger holds in a non-empty `Set`.
macro_rules! non_empty_vec_set {
    ($(#[$doc:meta])* $name:ident($element:ty)) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
        pub struct $name($crate::NonEmptyVecSet<$element>);

        impl std::ops::Deref for $name {
            type Target = [$element];

            fn deref(&self) -> &Self::Target {
                self.0.as_ref()
            }
        }

        impl<'b, C: $crate::cbor::HasProtocolVersion> $crate::cbor::Decode<'b, C> for $name {
            fn decode(
                d: &mut $crate::cbor::Decoder<'b>,
                ctx: &mut C,
            ) -> Result<Self, $crate::cbor::decode::Error> {
                Ok(Self(d.decode_with(ctx)?))
            }
        }

        impl<C: $crate::cbor::HasProtocolVersion> $crate::cbor::Encode<C> for $name {
            fn encode<W: $crate::cbor::encode::Write>(
                &self,
                e: &mut $crate::cbor::Encoder<W>,
                ctx: &mut C,
            ) -> Result<(), $crate::cbor::encode::Error<W::Error>> {
                e.encode_with(&self.0, ctx)?;
                Ok(())
            }
        }
    };
}

pub(crate) use non_empty_vec_set;

#[cfg(test)]
mod tests {
    use test_case::test_case;

    use super::NonEmptyVecSet;
    use crate::{from_cbor_no_leftovers, to_cbor};

    /// However the elements arrived, they are written as a tagged array in ascending order.
    #[test_case("83030102"; "out of order")]
    #[test_case("9f030102ff"; "indefinite-length")]
    #[test_case("d9010283010203"; "already tagged and ordered")]
    fn always_re_encodes_as_an_ascending_tagged_set(input: &str) {
        let set: NonEmptyVecSet<u8> = from_cbor_no_leftovers(&hex::decode(input).unwrap()).unwrap();

        assert_eq!(hex::encode(to_cbor(&set)), "d9010283010203");
    }

    #[test_case("80"; "the empty array")]
    #[test_case("d9010280"; "the empty tagged set")]
    #[test_case("830101"; "a repeated element")]
    fn rejects(input: &str) {
        assert!(from_cbor_no_leftovers::<NonEmptyVecSet<u8>>(&hex::decode(input).unwrap()).is_err());
    }
}
