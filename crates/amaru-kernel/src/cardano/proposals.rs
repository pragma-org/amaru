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

use minicbor::{Decoder, decode::Error};

use crate::{NonEmptyUniqueVec, Proposal, cbor};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Proposals(NonEmptyUniqueVec<Proposal>);

impl AsRef<[Proposal]> for Proposals {
    fn as_ref(&self) -> &[Proposal] {
        self.0.as_ref()
    }
}

impl IntoIterator for Proposals {
    type Item = Proposal;
    type IntoIter = std::vec::IntoIter<Proposal>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'b, C: cbor::HasProtocolVersion> cbor::Decode<'b, C> for Proposals {
    fn decode(d: &mut Decoder<'b>, ctx: &mut C) -> Result<Self, Error> {
        let proposals = NonEmptyUniqueVec::<Proposal>::decode(d, ctx)?;
        Ok(Self(proposals))
    }
}

impl<C: cbor::HasProtocolVersion> cbor::Encode<C> for Proposals {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut minicbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        self.0.encode(e, ctx)
    }
}

impl Proposals {
    pub fn iter(&self) -> std::slice::Iter<'_, Proposal> {
        self.0.iter()
    }
}
