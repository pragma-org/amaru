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

#[derive(Debug)]
#[repr(transparent)]
pub struct SerialisedAsArray<T>(pub T);

impl<'b, C, T: cbor::Decode<'b, C>> cbor::Decode<'b, C> for SerialisedAsArray<Option<T>> {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        let len = d.array()?;
        match len {
            Some(0) => Ok(Self(None)),
            Some(1) => d.decode_with(ctx).map(Some).map(Self),
            Some(_) => Err(cbor::decode::Error::message("too many elements in length-style decoding of Maybe")),
            None => {
                if cbor::decode_break(d, len)? {
                    Ok(Self(None))
                } else {
                    let value = d.decode_with(ctx)?;
                    if cbor::decode_break(d, len)? {
                        Ok(Self(Some(value)))
                    } else {
                        Err(cbor::decode::Error::message("too many elements in break-style decoding of Maybe"))
                    }
                }
            }
        }
    }
}

impl<C, T: cbor::Encode<C>> cbor::Encode<C> for SerialisedAsArray<Option<T>> {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        SerialisedAsArray(&self.0).encode(e, ctx)
    }
}

impl<C, T: cbor::Encode<C>> cbor::Encode<C> for SerialisedAsArray<&Option<T>> {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        match self.0.as_ref() {
            Some(value) => {
                e.array(1)?;
                value.encode(e, ctx)
            }
            None => {
                e.array(0)?;
                Ok(())
            }
        }
    }
}
