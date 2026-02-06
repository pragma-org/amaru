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

use minicbor::{Decode, Decoder, Encode};

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EraParams {
    pub epoch_size_slots: u64,
    pub slot_length: u64, // Milliseconds
}

impl EraParams {
    pub fn new(epoch_size_slots: u64, slot_length: u64) -> Option<Self> {
        if epoch_size_slots == 0 {
            return None;
        }
        if slot_length == 0 {
            return None;
        }
        Some(EraParams {
            epoch_size_slots,
            slot_length,
        })
    }
}

impl<C> Encode<C> for EraParams {
    fn encode<W: minicbor::encode::Write>(
        &self,
        e: &mut minicbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), minicbor::encode::Error<W::Error>> {
        e.begin_array()?;
        self.epoch_size_slots.encode(e, ctx)?;
        self.slot_length.encode(e, ctx)?;
        e.end()?;
        Ok(())
    }
}

impl<'b, C> Decode<'b, C> for EraParams {
    fn decode(d: &mut Decoder<'b>, _ctx: &mut C) -> Result<Self, minicbor::decode::Error> {
        let _ = d.array()?;
        let epoch_size_slots = d.decode()?;
        let slot_length = d.decode()?;
        d.skip()?;
        Ok(EraParams {
            epoch_size_slots,
            slot_length,
        })
    }
}
