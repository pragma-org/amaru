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

use std::collections::BTreeSet;

use amaru_kernel::{
    HasOwnership, MemoizedDatum, NonEmptyKeyValuePairs, ProposalId, RedeemerTag, RequiredScript, StakeCredential,
    Voter, VotingProcedure,
};
use itertools::Itertools;
use thiserror::Error;

use crate::context::{CommitteeSlice, DRepsSlice, PoolsSlice, ProposalsSlice, WitnessSlice};

#[derive(Debug, Error)]
pub enum InvalidVotingProcedures {
    #[error("voters do not exist: {0:?}")]
    VotersDoNotExist(BTreeSet<Voter>),

    #[error("governance actions do not exist: {0:?}")]
    GovActionsDoNotExist(BTreeSet<ProposalId>),
}

pub(crate) fn execute<C>(
    context: &mut C,
    voting_procedures: Option<NonEmptyKeyValuePairs<Voter, NonEmptyKeyValuePairs<ProposalId, VotingProcedure>>>,
) -> Result<(), InvalidVotingProcedures>
where
    C: WitnessSlice + ProposalsSlice + CommitteeSlice + DRepsSlice + PoolsSlice,
{
    if let Some(voting_procedures) = voting_procedures {
        let mut unknown_voters = BTreeSet::new();
        let mut unknown_proposals = BTreeSet::new();

        voting_procedures.into_iter().sorted_by_key(|(k, _)| *k).enumerate().for_each(|(index, (voter, votes))| {
            if !exists(context, &voter) {
                unknown_voters.insert(voter);
                return;
            }

            for (proposal_id, _) in votes.iter() {
                if !ProposalsSlice::exists(context, proposal_id, None) {
                    unknown_proposals.insert(*proposal_id);
                }
            }

            if !(unknown_voters.is_empty() && unknown_proposals.is_empty()) {
                return; // Skip validations after if any proposal or voter is invalid;
            }

            match voter.owner() {
                StakeCredential::ScriptHash(hash) => {
                    context.require_script_witness(RequiredScript {
                        hash,
                        index: index as u32,
                        purpose: RedeemerTag::Vote,
                        datum: MemoizedDatum::None,
                    });
                }
                StakeCredential::AddrKeyhash(hash) => context.require_verification_key_witness(hash),
            }

            votes.into_iter().for_each(|(proposal_id, ballot)| {
                context.vote(proposal_id, voter, ballot.vote, ballot.anchor);
            })
        });

        if !unknown_voters.is_empty() {
            return Err(InvalidVotingProcedures::VotersDoNotExist(unknown_voters));
        }

        if !unknown_proposals.is_empty() {
            return Err(InvalidVotingProcedures::GovActionsDoNotExist(unknown_proposals));
        }
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
            !CommitteeSlice::lookup_by_hot_credential(context, &voter.owner()).is_empty()
        }
        Voter::DRepKey(_) | Voter::DRepScript(_) => DRepsSlice::lookup(context, &voter.owner()).is_some(),
        Voter::StakePoolKey(pool) => PoolsSlice::exists(context, *pool),
    }
}
