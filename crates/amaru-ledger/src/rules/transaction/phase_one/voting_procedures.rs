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
    EraHistory, HasMajorVersion, HasOwnership, MemoizedDatum, NonEmptyKeyValuePairs, PROTOCOL_VERSION_10, ProposalId,
    ProtocolVersion, RedeemerTag, RequiredScript, StakeCredential, TransactionPointer, Voter, VotingProcedure,
};
use thiserror::Error;

use crate::context::{CommitteeSlice, DRepsSlice, PoolsSlice, ProposalsSlice, WitnessSlice};

#[derive(Debug, Error)]
pub enum InvalidVotingProcedures {
    #[error("vote cast by committee member who is not yet elected: {0:?}")]
    UnelectedCommitteeVoter(Voter),

    #[error("voters do not exist: {0:?}")]
    VotersDoNotExist(BTreeSet<Voter>),

    #[error("governance actions do not exist: {0:?}")]
    GovActionsDoNotExist(BTreeSet<ProposalId>),

    #[error("votes cast on governance actions that have expired: Voter {0:?} on proposal {1:?}")]
    VotingOnExpiredGovAction(Voter, ProposalId),

    #[error("era history error: {0}")]
    EraHistory(#[from] amaru_kernel::EraHistoryError),
}

pub(crate) fn execute<C>(
    context: &mut C,
    protocol_version: ProtocolVersion,
    era_history: &EraHistory,
    pointer: TransactionPointer,
    voting_procedures: Option<NonEmptyKeyValuePairs<Voter, NonEmptyKeyValuePairs<ProposalId, VotingProcedure>>>,
) -> Result<(), InvalidVotingProcedures>
where
    C: WitnessSlice + ProposalsSlice + CommitteeSlice + DRepsSlice + PoolsSlice,
{
    if let Some(voting_procedures) = voting_procedures {
        let voting_procedures = voting_procedures.into_iter().collect::<BTreeMap<_, _>>();

        // NOTE: conformance tests are brittle on this check due to era_history mismatch.
        // (see certificates.rs PoolRetirement comment for details)
        let current_epoch = era_history.slot_to_epoch(pointer.slot, pointer.slot)?;

        let mut unknown_voters = BTreeSet::new();
        let mut unknown_proposals = BTreeSet::new();

        for (voter, votes) in voting_procedures.iter() {
            if protocol_version.major() > PROTOCOL_VERSION_10.major() && is_unelected_committee_voter(context, voter) {
                return Err(InvalidVotingProcedures::UnelectedCommitteeVoter(voter.clone()));
            }

            if !exists(context, voter) {
                unknown_voters.insert(voter.clone());
            }

            for (proposal_id, _) in votes.iter() {
                match ProposalsSlice::lookup(context, proposal_id) {
                    None => {
                        unknown_proposals.insert(*proposal_id);
                    }

                    Some(state) if state.valid_until < current_epoch => {
                        return Err(InvalidVotingProcedures::VotingOnExpiredGovAction(voter.clone(), *proposal_id));
                    }

                    Some(..) => {}
                }
            }
        }

        if !unknown_voters.is_empty() {
            return Err(InvalidVotingProcedures::VotersDoNotExist(unknown_voters));
        }

        if !unknown_proposals.is_empty() {
            return Err(InvalidVotingProcedures::GovActionsDoNotExist(unknown_proposals));
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

/// Election statushere is membership, not an unexpired term: a member whose term has run out is still
/// named by the committee, and their vote is discounted when the action is ratified instead.
///
/// Voters that are not committee members are never rejected by this check.
fn is_unelected_committee_voter<C>(context: &C, voter: &Voter) -> bool
where
    C: CommitteeSlice,
{
    match voter {
        Voter::ConstitutionalCommitteeKey(_) | Voter::ConstitutionalCommitteeScript(_) => {
            !CommitteeSlice::lookup_by_hot_credential(context, &voter.owner())
                .iter()
                .any(|member| member.valid_until.is_some())
        }
        Voter::DRepKey(_) | Voter::DRepScript(_) | Voter::StakePoolKey(_) => false,
    }
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
