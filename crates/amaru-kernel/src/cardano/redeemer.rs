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

use crate::{ExUnits, MemoizedPlutusData, RedeemerTag, cbor};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, cbor::Encode)]
#[cbor(context_bound = "crate::cbor::HasProtocolVersion")]
pub struct Redeemer {
    #[n(0)]
    pub tag: RedeemerTag,

    #[n(1)]
    pub index: u32,

    #[n(2)]
    pub data: MemoizedPlutusData,

    #[n(3)]
    pub ex_units: ExUnits,
}

impl<'b, C: cbor::HasProtocolVersion> cbor::Decode<'b, C> for Redeemer {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        cbor::heterogeneous_array(d, |d, assert_len| {
            assert_len(4)?;
            let tag = d.decode_with(ctx)?;
            let index = d.decode_with(ctx)?;
            let data = d.decode_with(ctx)?;
            let ex_units = d.decode_with(ctx)?;
            Ok(Self { tag, index, data, ex_units })
        })
    }
}
