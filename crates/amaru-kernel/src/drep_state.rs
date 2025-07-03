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

use crate::{cbor, Anchor, Epoch, Lovelace, Set, StakeCredential, StrictMaybe};

#[derive(Debug)]
pub struct DRepState {
    pub expiry: Epoch,
    pub anchor: StrictMaybe<Anchor>,
    pub deposit: Lovelace,
    #[allow(dead_code)]
    pub delegators: Set<StakeCredential>,
}

impl<'b, C> cbor::decode::Decode<'b, C> for DRepState {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        d.array()?;
        Ok(DRepState {
            expiry: d.decode_with(ctx)?,
            anchor: d.decode_with(ctx)?,
            deposit: d.decode_with(ctx)?,
            delegators: d.decode_with(ctx)?,
        })
    }
}
