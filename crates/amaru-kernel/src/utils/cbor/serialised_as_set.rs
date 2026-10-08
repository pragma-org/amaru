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

use crate::cbor;

static TAG_SET: u64 = 258;

/// Encode an array-like type as a tagged set.
///
/// We make sure that elements are ordered before encoding in order to facilitate comparisons
/// with the Haskell node in tests.
#[derive(Debug)]
#[repr(transparent)]
pub struct SerialisedAsSet<T>(pub T);

impl<C, E: Ord + cbor::Encode<C>> cbor::Encode<C> for SerialisedAsSet<&[E]> {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.tag(cbor::Tag::new(TAG_SET))?;
        let mut sorted = self.0.iter().collect::<Vec<_>>();
        sorted.sort_unstable();
        e.encode_with(sorted.as_slice(), ctx)?;
        Ok(())
    }
}

impl<'d, C, T: cbor::Decode<'d, C>> cbor::Decode<'d, C> for SerialisedAsSet<T> {
    fn decode(d: &mut cbor::Decoder<'d>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        // TODO: In Conway, the tag is *optional*, with the intent to make it mandatory after.
        if d.datatype()? == cbor::Type::Tag {
            let tag = d.tag()?;
            if tag != cbor::Tag::new(TAG_SET) {
                return Err(cbor::decode::Error::message(format!("invalid set tag; expect={TAG_SET}, found={tag}")));
            }
        }

        Ok(Self(d.decode_with(ctx)?))
    }
}
