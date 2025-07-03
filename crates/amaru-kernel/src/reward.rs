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

use crate::{cbor, Lovelace, PoolId, RewardKind};

#[derive(Debug)]
pub struct Reward {
    #[allow(dead_code)]
    pub kind: RewardKind,
    #[allow(dead_code)]
    pub pool: PoolId,
    pub amount: Lovelace,
}

impl<'b, C> cbor::decode::Decode<'b, C> for Reward {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        d.array()?;
        let kind = d.decode_with(ctx)?;
        let pool = d.decode_with(ctx)?;
        let amount = d.decode_with(ctx)?;
        Ok(Reward { kind, pool, amount })
    }
}
