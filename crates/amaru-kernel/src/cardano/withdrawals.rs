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

use minicbor::{Decode, Decoder, Encode, decode, encode};

use crate::{Lovelace, NonEmptyKeyValuePairs, RewardAccount, cbor::HasProtocolVersion};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Withdrawals(NonEmptyKeyValuePairs<RewardAccount, Lovelace>);

impl IntoIterator for Withdrawals {
    type Item = (RewardAccount, Lovelace);
    type IntoIter = std::vec::IntoIter<(RewardAccount, Lovelace)>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'b, C: HasProtocolVersion> Decode<'b, C> for Withdrawals {
    fn decode(d: &mut Decoder<'b>, ctx: &mut C) -> Result<Self, decode::Error> {
        let withdrawals = NonEmptyKeyValuePairs::<RewardAccount, Lovelace>::decode(d, ctx)?;
        Ok(Self(withdrawals))
    }
}

impl<C: HasProtocolVersion> Encode<C> for Withdrawals {
    fn encode<W: minicbor::encode::Write>(
        &self,
        e: &mut minicbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), encode::Error<W::Error>> {
        self.0.encode(e, ctx)
    }
}

impl Withdrawals {
    pub fn iter(&self) -> std::slice::Iter<'_, (RewardAccount, Lovelace)> {
        self.0.iter()
    }
}
