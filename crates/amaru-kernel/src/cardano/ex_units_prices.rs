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

use std::fmt;

use crate::{RationalNumber, cbor};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, cbor::Encode)]
#[cbor(context_bound = "crate::cbor::HasProtocolVersion")]
pub struct ExUnitPrices {
    #[n(0)]
    pub mem_price: RationalNumber,

    #[n(1)]
    pub step_price: RationalNumber,
}

/// Decoded as a fixed-size record, so indefinite-length encodings are rejected before protocol version V12.
impl<'b, C: cbor::HasProtocolVersion> cbor::Decode<'b, C> for ExUnitPrices {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        cbor::record_v12_indefinite(d, ctx, 2, |d, ctx| {
            Ok(ExUnitPrices { mem_price: d.decode_with(ctx)?, step_price: d.decode_with(ctx)? })
        })
    }
}

impl fmt::Display for ExUnitPrices {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{{mem={}, cpu={}}}", self.mem_price, self.step_price)
    }
}
