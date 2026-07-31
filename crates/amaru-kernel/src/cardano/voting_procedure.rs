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

use std::collections::BTreeMap;

use crate::{Anchor, NonEmptyKeyValuePairs, ProposalId, Vote, Voter, cbor};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct VotingProcedure {
    pub vote: Vote,
    pub anchor: Option<Anchor>,
}

impl<'b, C> cbor::Decode<'b, C> for VotingProcedure {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        d.array()?;
        let vote = d.decode_with(ctx)?;
        let anchor = d.decode_with(ctx)?;
        Ok(Self { vote, anchor })
    }
}

impl<C> cbor::Encode<C> for VotingProcedure {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.array(2)?;
        e.encode_with(self.vote, ctx)?;
        e.encode_with(&self.anchor, ctx)?;
        Ok(())
    }
}

/// The governance votes cast by a transaction.
///
/// A nested map from [`Voter`] to the [`Vote`] (yes/no/abstain) it casts on each
/// governance action, keyed by [`ProposalId`]. Only the decision is kept,
/// the on-chain [`VotingProcedure`]'s anchor is dropped, since scripts never see it.
#[derive(Debug, Default)]
pub struct PlutusVotes<'a>(pub BTreeMap<&'a Voter, BTreeMap<ProposalId, &'a Vote>>);

impl<'a> From<&'a NonEmptyKeyValuePairs<Voter, NonEmptyKeyValuePairs<ProposalId, VotingProcedure>>>
    for PlutusVotes<'a>
{
    fn from(
        voting_procedures: &'a NonEmptyKeyValuePairs<Voter, NonEmptyKeyValuePairs<ProposalId, VotingProcedure>>,
    ) -> Self {
        Self(
            voting_procedures
                .iter()
                .map(|(voter, votes)| {
                    (voter, votes.iter().map(|(proposal, procedure)| (*proposal, &procedure.vote)).collect())
                })
                .collect(),
        )
    }
}
