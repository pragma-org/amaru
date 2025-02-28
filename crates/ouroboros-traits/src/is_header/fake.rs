// Copyright 2025 PRAGMA
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

use super::*;
use amaru_kernel::cbor;
use std::{
    fmt,
    fmt::{Display, Formatter},
};

/// Basic `Header` implementation for testing purposes.
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum FakeHeader {
    FakeHeader {
        block_number: u64,
        slot: u64,
        parent: Hash<HASH_SIZE>,
        body_hash: Hash<HASH_SIZE>,
    },
    Genesis,
}

impl IsHeader for FakeHeader {
    fn parent(&self) -> Option<Hash<HASH_SIZE>> {
        match self {
            Self::FakeHeader { parent, .. } => Some(*parent),
            Self::Genesis => None,
        }
    }

    fn block_height(&self) -> u64 {
        match self {
            Self::FakeHeader { block_number, .. } => *block_number,
            Self::Genesis => 0,
        }
    }

    fn slot(&self) -> u64 {
        match self {
            Self::FakeHeader { slot, .. } => *slot,
            Self::Genesis => 0,
        }
    }

    fn point(&self) -> Point {
        match self {
            Self::Genesis => Point::Origin,
            Self::FakeHeader { .. } => Point::Specific(self.slot(), self.hash().to_vec()),
        }
    }
}

impl Display for FakeHeader {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::FakeHeader {
                block_number,
                slot,
                parent,
                body_hash,
            } => {
                write!(
                f,
                "FakeHeader {{ hash: {}, block_number: {}, slot: {}, parent: {}, body_hash: {} }}",
                self.hash(), block_number, slot, parent, body_hash
            )
            }
            Self::Genesis => write!(f, "Genesis"),
        }
    }
}

impl<C> cbor::encode::Encode<C> for FakeHeader {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        match self {
            Self::FakeHeader {
                block_number,
                slot,
                parent,
                body_hash,
            } => e
                .encode(0)?
                .array(4)?
                .encode_with(block_number, ctx)?
                .encode_with(slot, ctx)?
                .encode_with(parent, ctx)?
                .encode_with(body_hash, ctx)?
                .ok(),
            Self::Genesis => e.encode(1)?.ok(),
        }
    }
}

impl<'b, C> cbor::decode::Decode<'b, C> for FakeHeader {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        let tag = d.u8()?;
        match tag {
            0 => {
                d.array()?;
                let block_number = d.decode_with(ctx)?;
                let slot = d.decode_with(ctx)?;
                let parent = d.decode_with(ctx)?;
                let body_hash = d.decode_with(ctx)?;
                Ok(Self::FakeHeader {
                    block_number,
                    slot,
                    parent,
                    body_hash,
                })
            }
            1 => Ok(Self::Genesis),
            _ => Err(cbor::decode::Error::message(format!("unknown tag {}", tag))),
        }
    }
}
