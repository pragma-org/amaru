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
    Hash, cbor,
    size::{KEY, POOL_COLD_KEY, SCRIPT},
};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub enum Voter {
    ConstitutionalCommitteeScript(Hash<{ SCRIPT }>),
    ConstitutionalCommitteeKey(Hash<{ KEY }>),
    DRepScript(Hash<{ SCRIPT }>),
    DRepKey(Hash<{ KEY }>),
    StakePoolKey(Hash<{ POOL_COLD_KEY }>),
}

impl<'b, C> cbor::decode::Decode<'b, C> for Voter {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        cbor::heterogeneous_array(d, |d, assert_len| {
            assert_len(2)?;
            let variant = d.u16()?;
            match variant {
                0 => Ok(Self::ConstitutionalCommitteeKey(d.decode_with(ctx)?)),
                1 => Ok(Self::ConstitutionalCommitteeScript(d.decode_with(ctx)?)),
                2 => Ok(Self::DRepKey(d.decode_with(ctx)?)),
                3 => Ok(Self::DRepScript(d.decode_with(ctx)?)),
                4 => Ok(Self::StakePoolKey(d.decode_with(ctx)?)),
                _ => Err(cbor::decode::Error::message("invalid variant id for Voter")),
            }
        })
    }
}

impl<C> cbor::encode::Encode<C> for Voter {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.array(2)?;

        match self {
            Self::ConstitutionalCommitteeKey(h) => {
                e.encode_with(0, ctx)?;
                e.encode_with(h, ctx)?;
            }

            Self::ConstitutionalCommitteeScript(h) => {
                e.encode_with(1, ctx)?;
                e.encode_with(h, ctx)?;
            }

            Self::DRepKey(h) => {
                e.encode_with(2, ctx)?;
                e.encode_with(h, ctx)?;
            }

            Self::DRepScript(h) => {
                e.encode_with(3, ctx)?;
                e.encode_with(h, ctx)?;
            }

            Self::StakePoolKey(h) => {
                e.encode_with(4, ctx)?;
                e.encode_with(h, ctx)?;
            }
        }

        Ok(())
    }
}
