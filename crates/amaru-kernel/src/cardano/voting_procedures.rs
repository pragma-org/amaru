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

use crate::{NonEmptyKeyValuePairs, ProposalId, Voter, VotingProcedure, cbor};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct VotingProcedures(NonEmptyKeyValuePairs<Voter, NonEmptyKeyValuePairs<ProposalId, VotingProcedure>>);

impl IntoIterator for VotingProcedures {
    type Item = (Voter, NonEmptyKeyValuePairs<ProposalId, VotingProcedure>);
    type IntoIter = std::vec::IntoIter<(Voter, NonEmptyKeyValuePairs<ProposalId, VotingProcedure>)>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'b, C: cbor::HasProtocolVersion> cbor::Decode<'b, C> for VotingProcedures {
    fn decode(d: &mut Decoder<'b>, ctx: &mut C) -> Result<Self, Error> {
        let voting_procedures =
            NonEmptyKeyValuePairs::<Voter, NonEmptyKeyValuePairs<ProposalId, VotingProcedure>>::decode(d, ctx)?;
        Ok(Self(voting_procedures))
    }
}

impl<C: cbor::HasProtocolVersion> cbor::Encode<C> for VotingProcedures {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut minicbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        self.0.encode(e, ctx)
    }
}

impl VotingProcedures {
    pub fn iter(&self) -> std::slice::Iter<'_, (Voter, NonEmptyKeyValuePairs<ProposalId, VotingProcedure>)> {
        self.0.iter()
    }
}
