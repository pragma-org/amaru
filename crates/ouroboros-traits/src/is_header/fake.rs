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
pub struct FakeHeader {
    pub block_number: u64,
    pub slot: u64,
    pub parent: Option<Hash<HASH_SIZE>>,
    pub body_hash: Hash<HASH_SIZE>,
}

impl IsHeader for FakeHeader {
    fn parent(&self) -> Option<Hash<HASH_SIZE>> {
        self.parent
    }

    fn block_height(&self) -> u64 {
        self.block_number
    }

    fn slot(&self) -> u64 {
        self.slot
    }

    fn point(&self) -> Point {
        Point::Specific(self.slot(), self.hash().to_vec())
    }

    fn extended_vrf_nonce_output(&self) -> Vec<u8> {
        unimplemented!(
            "called 'extended_vrf_nonce_output' on a Fake header clearly not ready for that."
        )
    }
}

impl Display for FakeHeader {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "FakeHeader {{ hash: {}, block_number: {}, slot: {}, parent: {}, body_hash: {} }}",
            self.hash(),
            self.block_number,
            self.slot,
            self.parent
                .map(|h| h.to_string())
                .unwrap_or_else(|| "None".to_string()),
            self.body_hash
        )
    }
}

impl<C> cbor::encode::Encode<C> for FakeHeader {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.array(4)?
            .encode_with(self.block_number, ctx)?
            .encode_with(self.slot, ctx)?
            .encode_with(self.parent, ctx)?
            .encode_with(self.body_hash, ctx)?
            .ok()
    }
}

impl<'b, C> cbor::decode::Decode<'b, C> for FakeHeader {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        d.array()?;
        let block_number = d.decode_with(ctx)?;
        let slot = d.decode_with(ctx)?;
        let parent = d.decode_with(ctx)?;
        let body_hash = d.decode_with(ctx)?;
        Ok(Self {
            block_number,
            slot,
            parent,
            body_hash,
        })
    }
}
