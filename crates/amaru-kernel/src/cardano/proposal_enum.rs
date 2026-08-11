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

use std::{cmp::Ordering, collections::BTreeMap, rc::Rc};

use crate::{
    Constitution, ConstitutionalCommitteeUpdate, GovernanceAction, OrphanProposal, ProposalId, ProtocolParamUpdate,
    ProtocolVersion, expect_stake_credential, into_safe_ratio,
};

/// Akin to a GovAction, but with a split that is more tailored to the ratification needs.
/// In particular:
///
/// - Motion of no confidence and update to the constitutional committee are grouped together as
///   `CommitteeUpdate`. This is because they, in fact, belong to the same chain of relationships.
///
/// - Treasury withdrawals and polls (a.k.a 'info actions') are also grouped together, as they're
///   the only actions that do not need to form a chain; they have no parents (hence,
///   `OrphanProposal`)
#[derive(Debug, Clone)]
pub enum ProposalEnum {
    ProtocolParameters(Box<ProtocolParamUpdate>, Option<Rc<ProposalId>>),
    HardFork(ProtocolVersion, Option<Rc<ProposalId>>),
    ConstitutionalCommittee(ConstitutionalCommitteeUpdate, Option<Rc<ProposalId>>),
    Constitution(Constitution, Option<Rc<ProposalId>>),
    Orphan(OrphanProposal),
}

impl ProposalEnum {
    /// The parent this proposal chains onto, if any. Orphan proposals never chain.
    pub fn parent(&self) -> Option<&ProposalId> {
        match self {
            Self::ProtocolParameters(_, parent)
            | Self::HardFork(_, parent)
            | Self::ConstitutionalCommittee(_, parent)
            | Self::Constitution(_, parent) => parent.as_deref(),
            Self::Orphan(_) => None,
        }
    }
    pub fn display_kind(&self) -> String {
        use ConstitutionalCommitteeUpdate::*;
        use OrphanProposal::*;

        match self {
            Self::ProtocolParameters(..) => "protocol-parameters",
            Self::HardFork(..) => "hard-fork",
            Self::ConstitutionalCommittee(NoConfidence, _) => "motion-of-no-confidence",
            Self::ConstitutionalCommittee(ChangeMembers { .. }, _) => "constitutional-committee",
            Self::Constitution(..) => "constitution",
            Self::Orphan(NicePoll) => "nice-poll",
            Self::Orphan(TreasuryWithdrawal(..)) => "treasury-withdrawal",
        }
        .to_string()
    }

    // Compare two proposals according to their priority. This influences the ratification order.
    //
    // 1st. NoConfidence
    // 2nd. UpdateCommittee
    // 3rd. NewConstitution
    // 4th. HardForkInitiation
    // 5th. ParameterChange
    // 6th. TreasuryWithdrawals
    // 7th. NicePoll
    pub fn cmp_priority(&self, other: &Self) -> Ordering {
        use ConstitutionalCommitteeUpdate::*;
        use Ordering::*;
        use OrphanProposal::*;

        match (self, other) {
            // Priority #1: No Confidence
            (Self::ConstitutionalCommittee(NoConfidence, ..), Self::ConstitutionalCommittee(NoConfidence, ..)) => Equal,
            (Self::ConstitutionalCommittee(NoConfidence, ..), _) => Greater,
            (_, Self::ConstitutionalCommittee(NoConfidence, ..)) => Less,
            // Priority #2: Update to the Constitutional Committee
            (Self::ConstitutionalCommittee(..), Self::ConstitutionalCommittee(..)) => Equal,
            (Self::ConstitutionalCommittee(..), _) => Greater,
            (_, Self::ConstitutionalCommittee(..)) => Less,
            // Priority #3: Update to the Constitution
            (Self::Constitution(..), Self::Constitution(..)) => Equal,
            (Self::Constitution(..), _) => Greater,
            (_, Self::Constitution(..)) => Less,
            // Priority #4: Hard Fork
            (Self::HardFork(..), Self::HardFork(..)) => Equal,
            (Self::HardFork(..), _) => Greater,
            (_, Self::HardFork(..)) => Less,
            // Priority #5: Protocol Parameters updates
            (Self::ProtocolParameters(..), Self::ProtocolParameters(..)) => Equal,
            (Self::ProtocolParameters(..), _) => Greater,
            (_, Self::ProtocolParameters(..)) => Less,
            // Priority #6: Treasury Withdrawals
            (Self::Orphan(TreasuryWithdrawal(..)), Self::Orphan(TreasuryWithdrawal(..))) => Equal,
            (Self::Orphan(TreasuryWithdrawal(..)), _) => Greater,
            (_, Self::Orphan(TreasuryWithdrawal(..))) => Less,
            // Priority #7: Nice polls
            (Self::Orphan(NicePoll), Self::Orphan(NicePoll)) => Equal,
        }
    }

    pub fn is_hardfork(&self) -> bool {
        matches!(self, Self::HardFork(..))
    }

    pub fn is_orphan(&self) -> bool {
        matches!(self, Self::Orphan(..))
    }

    pub fn is_no_confidence(&self) -> bool {
        use ConstitutionalCommitteeUpdate::*;
        matches!(self, Self::ConstitutionalCommittee(NoConfidence, _))
    }

    pub fn is_committee_member_update(&self) -> bool {
        use ConstitutionalCommitteeUpdate::*;
        matches!(self, Self::ConstitutionalCommittee(ChangeMembers { .. }, _))
    }
    pub fn is_nice_poll(&self) -> bool {
        matches!(self, Self::Orphan(OrphanProposal::NicePoll))
    }
}

impl From<GovernanceAction> for ProposalEnum {
    fn from(action: GovernanceAction) -> Self {
        use GovernanceAction::*;
        use OrphanProposal::*;

        match action {
            ParameterChange(parent, update, _guardrails_script) => {
                Self::ProtocolParameters(update, parent.map(Rc::new))
            }

            HardForkInitiation(parent, protocol_version) => Self::HardFork(protocol_version, parent.map(Rc::new)),

            TreasuryWithdrawals(withdrawals, _guardrails_script) => {
                let withdrawals =
                    withdrawals.into_iter().fold(BTreeMap::new(), |mut accum, (reward_account, amount)| {
                        accum.insert(expect_stake_credential(&reward_account), amount);
                        accum
                    });

                Self::Orphan(TreasuryWithdrawal(withdrawals))
            }

            UpdateCommittee(parent, removed, added, threshold) => Self::ConstitutionalCommittee(
                ConstitutionalCommitteeUpdate::ChangeMembers {
                    removed: removed.into_iter().collect(),
                    added: added.into_iter().collect(),
                    threshold: into_safe_ratio(&threshold),
                },
                parent.map(Rc::new),
            ),

            NoConfidence(parent) => {
                Self::ConstitutionalCommittee(ConstitutionalCommitteeUpdate::NoConfidence, parent.map(Rc::new))
            }

            NewConstitution(parent, constitution) => Self::Constitution(constitution, parent.map(Rc::new)),

            Information => Self::Orphan(NicePoll),
        }
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub use tests::*;

#[cfg(any(test, feature = "test-utils"))]
mod tests {
    use std::rc::Rc;

    use proptest::{option, prelude::*};

    use crate::{
        ProposalEnum, any_constitution, any_constitutional_committee_update, any_epoch, any_orphan_proposal,
        any_proposal_id, any_protocol_params_update, any_protocol_version,
    };

    pub fn any_proposal_enum() -> impl Strategy<Value = ProposalEnum> {
        let any_protocol_parameters =
            (option::of(any_proposal_id()), any_protocol_params_update()).prop_map(|(parent, params_update)| {
                ProposalEnum::ProtocolParameters(Box::new(params_update), parent.map(Rc::new))
            });

        let any_hard_fork = (option::of(any_proposal_id()), any_protocol_version())
            .prop_map(|(parent, protocol_version)| ProposalEnum::HardFork(protocol_version, parent.map(Rc::new)));

        let any_constitutional_committee =
            (option::of(any_proposal_id()), any_constitutional_committee_update(any_epoch()))
                .prop_map(|(parent, committee)| ProposalEnum::ConstitutionalCommittee(committee, parent.map(Rc::new)));

        let any_constitution = (option::of(any_proposal_id()), any_constitution())
            .prop_map(|(parent, constitution)| ProposalEnum::Constitution(constitution, parent.map(Rc::new)));

        let any_orphan = any_orphan_proposal().prop_map(ProposalEnum::Orphan);

        prop_oneof![any_protocol_parameters, any_hard_fork, any_constitutional_committee, any_constitution, any_orphan,]
    }
}
