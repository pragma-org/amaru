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

use crate::{ExUnits, MemoizedPlutusData, cbor};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, cbor::Encode)]
#[cbor(context_bound = "crate::cbor::HasProtocolVersion")]
pub struct RedeemerValue {
    #[n(0)]
    pub data: MemoizedPlutusData,
    #[n(1)]
    pub ex_units: ExUnits,
}

impl<'b, C: cbor::HasProtocolVersion> cbor::Decode<'b, C> for RedeemerValue {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        cbor::record_v12_indefinite(d, ctx, 2, |d, ctx| {
            Ok(Self { data: d.decode_with(ctx)?, ex_units: d.decode_with(ctx)? })
        })
    }
}
