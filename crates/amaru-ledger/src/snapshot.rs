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

use amaru_kernel::{
    Account, CertificatePointer, ConstitutionalCommittee, ConstitutionalCommitteeMemberStatus, EraHistory,
    EraHistoryError, Lovelace, Point, ProposalId, ProposalPointer, ProposalState as NewEpochProposalState,
    ProposalsRoots, ProtocolParameters, Slot, StakeCredential, TransactionPointer,
};

use crate::context::{AccountState, CCMember, ProposalState};

/// An account's block-start state from a snapshot. The delegation pointers are synthesized, since a
/// NewEpochState records balances and delegations but not the certificates that set them.
pub fn account_state(
    account: Account,
    rewards_update: Lovelace,
    point: &Point,
    protocol_parameters: &ProtocolParameters,
) -> AccountState {
    let (rewards, deposit) = account.rewards_and_deposit.unwrap_or((0, protocol_parameters.stake_credential_deposit));

    let pool = account.pool.map(|pool| {
        let pointer = CertificatePointer {
            transaction: TransactionPointer { slot: Slot::from(0), transaction_index: 0 },
            certificate_index: 0,
        };
        (pool, pointer)
    });

    let drep = account.drep.map(|drep| {
        let pointer = CertificatePointer {
            transaction: TransactionPointer { slot: point.slot_or_default(), ..TransactionPointer::default() },
            certificate_index: 1,
        };
        (drep, pointer)
    });

    AccountState { deposit, pool, drep, rewards: rewards + rewards_update }
}

/// Each committee member's declared hot key and term as of the snapshot. A no-confidence committee
/// has no elected members.
pub fn committee_members(
    cc: Option<ConstitutionalCommittee>,
    hot_cold_delegations: &BTreeMap<StakeCredential, ConstitutionalCommitteeMemberStatus>,
) -> BTreeMap<StakeCredential, CCMember> {
    let members = match cc {
        Some(ConstitutionalCommittee { members, .. }) => members,
        None => return BTreeMap::new(),
    };

    members
        .into_iter()
        .map(|(cold_credential, valid_until)| {
            let hot_credential = match hot_cold_delegations.get(&cold_credential) {
                Some(ConstitutionalCommitteeMemberStatus::DelegatedToHotCredential(hot)) => Some(*hot),
                None | Some(ConstitutionalCommitteeMemberStatus::Resigned(..)) => None,
            };
            (cold_credential, CCMember { hot_credential, valid_until: Some(valid_until) })
        })
        .collect()
}

/// A governance proposal's block-start state from a snapshot. `proposed_in` is synthesized to the
/// start of its proposing epoch, the only position a NewEpochState records.
pub fn proposal_state(
    proposal: NewEpochProposalState,
    era_history: &EraHistory,
    protocol_parameters: &ProtocolParameters,
) -> Result<(ProposalId, ProposalState), EraHistoryError> {
    let NewEpochProposalState { id, procedure, proposed_in, .. } = proposal;

    let proposed_in_pointer = ProposalPointer {
        transaction: TransactionPointer { slot: era_history.epoch_bounds(proposed_in)?.start, transaction_index: 0 },
        proposal_index: id.action_index as usize,
    };

    Ok((
        id,
        ProposalState {
            proposed_in: proposed_in_pointer,
            valid_until: proposed_in + protocol_parameters.gov_action_lifetime,
            proposal: procedure,
        },
    ))
}

/// The governance roots, the latest enacted action per category, as of the snapshot.
pub fn proposals_roots(
    protocol_parameters: Option<ProposalId>,
    hard_fork: Option<ProposalId>,
    constitutional_committee: Option<ProposalId>,
    constitution: Option<ProposalId>,
) -> ProposalsRoots {
    ProposalsRoots { protocol_parameters, hard_fork, constitutional_committee, constitution }
}
