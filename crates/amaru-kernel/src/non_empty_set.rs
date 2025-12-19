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
    Deref, KeepRaw,
    cbor::{Decode, Encode, data::Tag, data::Type, decode::Error},
};

use serde::{Deserialize, Serialize};

static TAG_SET: u64 = 258;

/// Non-empty Set
///
/// Optional 258 tag (until era after Conway, at which point is it required)
/// with a vec of items which should contain no duplicates
#[derive(Debug, PartialEq, Eq, Clone, PartialOrd, Serialize, Deserialize)]
pub struct NonEmptySet<T>(Vec<T>);

impl<T> NonEmptySet<T> {
    pub fn to_vec(self) -> Vec<T> {
        self.0
    }

    pub fn from_vec(x: Vec<T>) -> Option<Self> {
        if x.is_empty() { None } else { Some(Self(x)) }
    }
}

impl<T> Deref for NonEmptySet<T> {
    type Target = Vec<T>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> TryFrom<Vec<T>> for NonEmptySet<T> {
    type Error = Vec<T>;

    fn try_from(value: Vec<T>) -> Result<Self, Self::Error> {
        if value.is_empty() {
            Err(value)
        } else {
            Ok(NonEmptySet(value))
        }
    }
}

impl<T> From<NonEmptySet<KeepRaw<'_, T>>> for NonEmptySet<T> {
    fn from(value: NonEmptySet<KeepRaw<'_, T>>) -> Self {
        let inner = value.0.into_iter().map(|x| x.unwrap()).collect();
        Self(inner)
    }
}

impl<'a, T> IntoIterator for &'a NonEmptySet<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl<'b, C, T> minicbor::decode::Decode<'b, C> for NonEmptySet<T>
where
    T: Decode<'b, C>,
{
    fn decode(d: &mut minicbor::Decoder<'b>, ctx: &mut C) -> Result<Self, minicbor::decode::Error> {
        // decode optional set tag (this will be required in era following Conway)
        if d.datatype()? == Type::Tag {
            let found_tag = d.tag()?;

            if found_tag != Tag::new(TAG_SET) {
                return Err(Error::message(format!("Unrecognised tag: {found_tag:?}")));
            }
        }

        let inner: Vec<T> = d.decode_with(ctx)?;

        if inner.is_empty() {
            return Err(Error::message("decoding empty set as NonEmptySet"));
        }

        Ok(Self(inner))
    }
}

impl<C, T> minicbor::encode::Encode<C> for NonEmptySet<T>
where
    T: Encode<C>,
{
    fn encode<W: minicbor::encode::Write>(
        &self,
        e: &mut minicbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), minicbor::encode::Error<W::Error>> {
        e.tag(Tag::new(TAG_SET))?;
        e.encode_with(&self.0, ctx)?;

        Ok(())
    }
}
