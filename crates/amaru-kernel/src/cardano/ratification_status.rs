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

/// A self-documenting boolean
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RatificationStatus {
    Ratified,
    NotRatified,
}

impl<C> cbor::Encode<C> for RatificationStatus {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.encode_with(
            match self {
                Self::Ratified => true,
                Self::NotRatified => false,
            },
            ctx,
        )?;
        Ok(())
    }
}

impl<'b, C> cbor::Decode<'b, C> for RatificationStatus {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        Ok(if bool::decode(d, ctx)? { Self::Ratified } else { Self::NotRatified })
    }
}
