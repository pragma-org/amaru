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

use crate::{EraName, cbor, utils::cbor::SerialisedAsMillis};
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EraParams {
    pub epoch_size_slots: u64,
    #[serde(deserialize_with = "SerialisedAsMillis::deserialize")]
    #[serde(serialize_with = "SerialisedAsMillis::serialize")]
    pub slot_length: Duration,
    pub era_name: EraName,
}

impl EraParams {
    pub fn new(epoch_size_slots: u64, slot_length: Duration, era_name: EraName) -> Option<Self> {
        if epoch_size_slots == 0 {
            return None;
        }
        if slot_length.is_zero() {
            return None;
        }
        Some(EraParams {
            epoch_size_slots,
            slot_length,
            era_name,
        })
    }
}

impl<C> cbor::Encode<C> for EraParams {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.array(3)?;
        self.epoch_size_slots.encode(e, ctx)?;
        SerialisedAsMillis::from(self.slot_length).encode(e, ctx)?;
        self.era_name.encode(e, ctx)?;
        Ok(())
    }
}

impl<'b, C> cbor::Decode<'b, C> for EraParams {
    fn decode(d: &mut cbor::Decoder<'b>, _ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        let len = d.array()?;
        if len != Some(3) {
            return Err(cbor::decode::Error::message(format!(
                "Expected 3 elements in EraParams, got {len:?}"
            )));
        }
        let epoch_size_slots = d.decode()?;
        let slot_length: SerialisedAsMillis = d.decode()?;
        let era_name = d.decode()?;
        Ok(EraParams {
            epoch_size_slots,
            slot_length: Duration::from(slot_length),
            era_name,
        })
    }
}
