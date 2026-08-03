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

use crate::context::{CommitteeSlice, DRepsSlice, PoolsSlice, ProposalsSlice, WitnessSlice};

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
    C: WitnessSlice + ProposalsSlice + CommitteeSlice + DRepsSlice + PoolsSlice,
{
    if let Some(voting_procedures) = voting_procedures {
        let voting_procedures = voting_procedures.into_iter().collect::<BTreeMap<_, _>>();

        let unknown_voters =
            voting_procedures.keys().filter(|voter| !exists(context, voter)).cloned().collect::<BTreeSet<_>>();

        if !unknown_voters.is_empty() {
            return Err(InvalidVotingProcedures::VotersDoNotExist(unknown_voters));
        }

        voting_procedures.into_iter().enumerate().for_each(|(index, (voter, votes))| {
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
        });
    }

    Ok(())
}

/// Whether the entity a vote is cast by is known at this point in the block.
fn exists<C>(context: &C, voter: &Voter) -> bool
where
    C: CommitteeSlice + DRepsSlice + PoolsSlice,
{
    match voter {
        // A vote identifies its member by the hot credential the member authorized, never by the cold
        // credential that identifies the seat.
        Voter::ConstitutionalCommitteeKey(_) | Voter::ConstitutionalCommitteeScript(_) => {
            CommitteeSlice::lookup_by_hot_credential(context, &voter.owner()).is_some()
        }
        Voter::DRepKey(_) | Voter::DRepScript(_) => DRepsSlice::lookup(context, &voter.owner()).is_some(),
        Voter::StakePoolKey(pool) => PoolsSlice::exists(context, *pool),
    }
}
