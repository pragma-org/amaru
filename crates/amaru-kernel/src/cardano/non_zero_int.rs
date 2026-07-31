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

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, std::hash::Hash, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct NonZeroInt(i64);

impl TryFrom<i64> for NonZeroInt {
    type Error = i64;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        if value == 0 {
            return Err(value);
        }

        Ok(Self(value))
    }
}

impl From<NonZeroInt> for i64 {
    fn from(value: NonZeroInt) -> Self {
        value.0
    }
}

impl From<&NonZeroInt> for i64 {
    fn from(x: &NonZeroInt) -> Self {
        i64::from(*x)
    }
}

impl<'b, C> cbor::Decode<'b, C> for NonZeroInt {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        let n = d.decode_with(ctx)?;

        if n == 0 {
            return Err(cbor::decode::Error::message("decoding 0 as NonZeroInt"));
        }

        Ok(Self(n))
    }
}

impl<C> cbor::Encode<C> for NonZeroInt {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        _ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.encode(self.0)?;

        Ok(())
    }
}
