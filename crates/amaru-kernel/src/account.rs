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

use crate::{cbor, DRep, Lovelace, PoolId, Set, StrictMaybe};

#[derive(Debug)]
pub struct Account {
    pub rewards_and_deposit: StrictMaybe<(Lovelace, Lovelace)>,
    #[allow(dead_code)]
    pub pointers: Set<(u64, u64, u64)>,
    pub pool: StrictMaybe<PoolId>,
    pub drep: StrictMaybe<DRep>,
}

impl<'b, C> cbor::decode::Decode<'b, C> for Account {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        d.array()?;
        Ok(Account {
            rewards_and_deposit: d.decode_with(ctx)?,
            pointers: d.decode_with(ctx)?,
            pool: d.decode_with(ctx)?,
            drep: d.decode_with(ctx)?,
        })
    }
}
