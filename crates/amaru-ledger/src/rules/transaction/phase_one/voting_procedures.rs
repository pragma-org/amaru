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
    Epoch, EraHistory, HasOwnership, MemoizedDatum, NonEmptyKeyValuePairs, ProposalId, ProposalSlim, ProtocolVersion,
    RedeemerTag, RequiredScript, StakeCredential, TransactionPointer, Voter, VotingProcedure,
    protocol_version::PROTOCOL_VERSION_11, utils::string::display_map,
};
use itertools::Itertools;
use thiserror::Error;

use crate::context::{CommitteeSlice, DRepsSlice, PoolsSlice, ProposalStateSlim, ProposalsSlice, WitnessSlice};

#[derive(Debug, Error)]
pub enum InvalidVotingProcedures {
    #[error("unauthorized or unknown voters: {0:?}")]
    UnknownVoter(BTreeSet<Voter>),

    #[error("invalid combinations of voters and proposals they have no say over: {}", display_map(.0))]
    DisallowedVoter(BTreeMap<Voter, ProposalSlim>),

    #[error("governance actions do not exist: {0:?}")]
    GovActionsDoNotExist(BTreeSet<ProposalId>),

    #[error("votes cast on governance actions that have expired: {}", display_map(.0))]
    VotingOnExpiredGovAction(BTreeMap<Voter, ProposalId>),

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
        // NOTE: Some conformance tests fail this check because the Haskell imp tests run on a
        // synthetic test chain whose epoch/slot mapping differs from our era_history. Our
        // slot_to_epoch computes a different current epoch, so an action they expect to have expired
        // still reads as live here.
        let current_epoch = era_history.slot_to_epoch(pointer.slot, pointer.slot)?;

        let mut unknown_voters = BTreeSet::new();
        let mut unknown_proposals = BTreeSet::new();
        let mut expired_proposals = BTreeMap::new();
        let mut disallowed_voters = BTreeMap::new();

        voting_procedures.into_iter().sorted_by_key(|(k, _)| *k).enumerate().for_each(|(index, (voter, votes))| {
            if !is_known_voter(context, protocol_version, &voter) {
                unknown_voters.insert(voter);
                return;
            }

            for (proposal_id, _) in votes.iter() {
                match ProposalsSlice::lookup(context, proposal_id) {
                    None => {
                        unknown_proposals.insert(*proposal_id);
                    }
                    Some(proposal) => {
                        if is_expired(&proposal, current_epoch) {
                            expired_proposals.insert(voter, *proposal_id);
                            return;
                        }

                        if !is_allowed_voter(&voter, &proposal.action) {
                            disallowed_voters.insert(voter, proposal.action);
                            return;
                        }
                    }
                }
            }

            if !(unknown_voters.is_empty()
                && disallowed_voters.is_empty()
                && unknown_proposals.is_empty()
                && expired_proposals.is_empty())
            {
                return;
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
                StakeCredential::KeyHash(hash) => context.require_verification_key_witness(hash),
            }

            votes.into_iter().for_each(|(proposal_id, ballot)| {
                context.vote(proposal_id, voter, ballot.vote, ballot.anchor);
            })
        });

        if !unknown_voters.is_empty() {
            return Err(InvalidVotingProcedures::UnknownVoter(unknown_voters));
        }

        if !unknown_proposals.is_empty() {
            return Err(InvalidVotingProcedures::GovActionsDoNotExist(unknown_proposals));
        }

        if !expired_proposals.is_empty() {
            return Err(InvalidVotingProcedures::VotingOnExpiredGovAction(expired_proposals));
        }

        if !disallowed_voters.is_empty() {
            return Err(InvalidVotingProcedures::DisallowedVoter(disallowed_voters));
        }
    }

    Ok(())
}

/// Whether the proposal is past the last epoch in which a vote on it still counts.
fn is_expired(proposal: &ProposalStateSlim, current_epoch: Epoch) -> bool {
    proposal.valid_until < current_epoch
}

/// Whether a voter has any say over this kind of governance action
fn is_allowed_voter(voter: &Voter, proposal: &ProposalSlim) -> bool {
    use ProposalSlim::*;
    use Voter::*;

    match voter {
        DRepKey(..) | DRepScript(..) => true,

        ConstitutionalCommitteeKey(..) | ConstitutionalCommitteeScript(..) => match proposal {
            ConstitutionalCommittee => false,
            ProtocolParameters(..) | Constitution | Orphan(..) | HardFork(..) => true,
        },

        StakePoolKey(..) => match proposal {
            ProtocolParameters(any_in_security_group) => bool::from(*any_in_security_group),
            Orphan(is_treasury_withdrawals) => !bool::from(*is_treasury_withdrawals),
            ConstitutionalCommittee | HardFork(..) => true,
            Constitution => false,
        },
    }
}

/// Whether the entity a vote is cast by is known at this point in the block.
fn is_known_voter<C>(context: &C, protocol_version: ProtocolVersion, voter: &Voter) -> bool
where
    C: CommitteeSlice + DRepsSlice + PoolsSlice,
{
    use Voter::*;

    match voter {
        // A vote identifies its member by the hot credential the member authorized, never by the cold
        // credential that identifies the seat.
        ConstitutionalCommitteeKey(_) | ConstitutionalCommitteeScript(_) => {
            let owner = voter.owner();
            let mut cc_members = CommitteeSlice::lookup_by_hot_credential(context, &owner);
            if protocol_version >= PROTOCOL_VERSION_11 {
                cc_members.any(|cc_member| cc_member.valid_until.is_some())
            } else {
                cc_members.count() > 0
            }
        }
        DRepKey(_) | DRepScript(_) => DRepsSlice::lookup(context, &voter.owner()).is_some(),
        StakePoolKey(pool) => PoolsSlice::exists(context, *pool),
    }
}
