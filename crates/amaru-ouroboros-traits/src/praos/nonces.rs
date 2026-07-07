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

use amaru_kernel::{Epoch, HeaderHash, Nonce, cbor};

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, serde::Serialize, serde::Deserialize)]
pub struct Nonces {
    pub active: Nonce,
    pub evolving: Nonce,
    pub candidate: Nonce,
    pub tail: HeaderHash,
    pub epoch: Epoch,
}

impl<C> cbor::encode::Encode<C> for Nonces {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.begin_array()?;
        e.encode_with(self.active, ctx)?;
        e.encode_with(self.evolving, ctx)?;
        e.encode_with(self.candidate, ctx)?;
        e.encode_with(self.tail, ctx)?;
        e.encode_with(self.epoch, ctx)?;
        e.end()?;
        Ok(())
    }
}

impl<'b, C> cbor::decode::Decode<'b, C> for Nonces {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        d.array()?;
        Ok(Nonces {
            active: d.decode_with(ctx)?,
            evolving: d.decode_with(ctx)?,
            candidate: d.decode_with(ctx)?,
            tail: d.decode_with(ctx)?,
            epoch: d.decode_with(ctx)?,
        })
    }
}
