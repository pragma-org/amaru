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

use std::collections::{BTreeMap, BTreeSet};

use amaru_kernel::{
    HasOwnership, MemoizedDatum, NonEmptyKeyValuePairs, ProposalId, RedeemerTag, RequiredScript, StakeCredential,
    Voter, VotingProcedure,
};
use thiserror::Error;

use crate::context::{ProposalsSlice, WitnessSlice};

#[derive(Debug, Error)]
pub enum InvalidVotingProcedures {
    #[error("voters do not exist: {0:?}")]
    VotersDoNotExist(BTreeSet<Voter>),
}

pub(crate) fn execute<C>(
    context: &mut C,
    voting_procedures: Option<NonEmptyKeyValuePairs<Voter, NonEmptyKeyValuePairs<ProposalId, VotingProcedure>>>,
) -> Result<(), InvalidVotingProcedures>
where
    C: WitnessSlice + ProposalsSlice,
{
    if let Some(voting_procedures) = voting_procedures {
        voting_procedures.into_iter().collect::<BTreeMap<_, _>>().into_iter().enumerate().for_each(
            |(index, (voter, votes))| {
                match voter.owner() {
                    StakeCredential::ScriptHash(hash) => {
                        context.require_script_witness(RequiredScript {
                            hash,
                            index: index as u32,
                            purpose: RedeemerTag::Vote,
                            datum: MemoizedDatum::None,
                        });
                    }
                    StakeCredential::AddrKeyhash(hash) => context.require_vkey_witness(hash),
                }

                votes.into_iter().for_each(|(proposal_id, ballot)| {
                    context.vote(proposal_id, voter.clone(), ballot.vote, ballot.anchor);
                })
            },
        );
    }

    Ok(())
}
