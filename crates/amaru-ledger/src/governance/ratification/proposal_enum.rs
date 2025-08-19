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

use crate::summary::SafeRatio;
use amaru_kernel::{
    ComparableProposalId, Constitution, Epoch, Lovelace, ProtocolParamUpdate, ProtocolVersion,
    StakeCredential,
};
use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
    fmt,
    rc::Rc,
};

// ProposalEnum
// ----------------------------------------------------------------------------

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
    ProtocolParameters(ProtocolParamUpdate, Option<Rc<ComparableProposalId>>),
    HardFork(ProtocolVersion, Option<Rc<ComparableProposalId>>),
    ConstitutionalCommittee(CommitteeUpdate, Option<Rc<ComparableProposalId>>),
    Constitution(Constitution, Option<Rc<ComparableProposalId>>),
    Orphan(OrphanProposal),
}

impl ProposalEnum {
    pub fn display_kind(&self) -> String {
        match self {
            Self::ProtocolParameters(..) => "protocol-parameters",
            Self::HardFork(..) => "hard-fork",
            Self::ConstitutionalCommittee(CommitteeUpdate::NoConfidence, _) => {
                "motion-of-no-confidence"
            }
            Self::ConstitutionalCommittee(CommitteeUpdate::ChangeMembers { .. }, _) => {
                "constitutional-committee"
            }
            Self::Constitution(..) => "constitution",
            Self::Orphan(OrphanProposal::NicePoll) => "nice-poll",
            Self::Orphan(OrphanProposal::TreasuryWithdrawal(..)) => "treasury-withdrawal",
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
    // 7th. Information
    pub fn cmp_priority(&self, other: &Self) -> Ordering {
        use CommitteeUpdate::*;
        use OrphanProposal::*;
        use ProposalEnum::*;

        match (self, other) {
            // Priority #1: No Confidence
            (
                ConstitutionalCommittee(NoConfidence, ..),
                ConstitutionalCommittee(NoConfidence, ..),
            ) => Ordering::Equal,
            (ConstitutionalCommittee(NoConfidence, ..), _) => Ordering::Greater,
            (_, ConstitutionalCommittee(NoConfidence, ..)) => Ordering::Less,
            // Priority #2: Update to the Constitutional Committee
            (ConstitutionalCommittee(..), ConstitutionalCommittee(..)) => Ordering::Equal,
            (ConstitutionalCommittee(..), _) => Ordering::Greater,
            (_, ConstitutionalCommittee(..)) => Ordering::Less,
            // Priority #3: Update to the Constitution
            (Constitution(..), Constitution(..)) => Ordering::Equal,
            (Constitution(..), _) => Ordering::Greater,
            (_, Constitution(..)) => Ordering::Less,
            // Priority #4: Hard Fork
            (HardFork(..), HardFork(..)) => Ordering::Equal,
            (HardFork(..), _) => Ordering::Greater,
            (_, HardFork(..)) => Ordering::Less,
            // Priority #5: Protocol Parameters updates
            (ProtocolParameters(..), ProtocolParameters(..)) => Ordering::Equal,
            (ProtocolParameters(..), _) => Ordering::Greater,
            (_, ProtocolParameters(..)) => Ordering::Less,
            // Priority #6: Treasury Withdrawals
            (Orphan(TreasuryWithdrawal(..)), Orphan(TreasuryWithdrawal(..))) => Ordering::Equal,
            (Orphan(TreasuryWithdrawal(..)), _) => Ordering::Greater,
            (_, Orphan(TreasuryWithdrawal(..))) => Ordering::Less,
            // Priority #7: Nice polls
            (Orphan(NicePoll), Orphan(NicePoll)) => Ordering::Equal,
        }
    }

    pub fn is_hardfork(&self) -> bool {
        matches!(self, Self::HardFork(..))
    }

    pub fn is_no_confidence(&self) -> bool {
        use CommitteeUpdate::*;
        matches!(self, Self::ConstitutionalCommittee(NoConfidence, _))
    }

    pub fn is_committee_member_update(&self) -> bool {
        use CommitteeUpdate::*;
        matches!(self, Self::ConstitutionalCommittee(ChangeMembers { .. }, _))
    }
}

// CommitteeUpdate
// ----------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum CommitteeUpdate {
    NoConfidence,
    ChangeMembers {
        removed: BTreeSet<StakeCredential>,
        added: BTreeMap<StakeCredential, Epoch>,
        threshold: SafeRatio,
    },
}

impl fmt::Display for CommitteeUpdate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoConfidence => write!(f, "no-confidence"),
            Self::ChangeMembers {
                removed,
                added,
                threshold,
            } => {
                let mut need_separator = false;

                if !removed.is_empty() {
                    write!(f, "{} removed", removed.len())?;
                    need_separator = true;
                }

                if !added.is_empty() {
                    write!(
                        f,
                        "{}{} added",
                        if need_separator { ", " } else { "" },
                        added.len()
                    )?;
                    need_separator = true;
                }

                write!(
                    f,
                    "{}threshold={}/{}",
                    if need_separator { ", " } else { "" },
                    threshold.numer(),
                    threshold.denom(),
                )
            }
        }
    }
}

// OrphanProposal
// ----------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum OrphanProposal {
    TreasuryWithdrawal(BTreeMap<StakeCredential, Lovelace>),
    NicePoll,
}

impl fmt::Display for OrphanProposal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OrphanProposal::NicePoll => write!(f, "nice poll"),
            OrphanProposal::TreasuryWithdrawal(withdrawals) => {
                let total = withdrawals
                    .iter()
                    .fold(0, |total, (_, single)| total + single)
                    / 1_000_000;
                write!(f, "withdrawal={total}₳")
            }
        }
    }
}

// Tests
// ----------------------------------------------------------------------------

#[cfg(any(test, feature = "test-utils"))]
pub mod tests {
    use super::{CommitteeUpdate, OrphanProposal, ProposalEnum};
    use crate::summary::SafeRatio;
    use amaru_kernel::{
        tests::{
            any_comparable_proposal_id, any_constitution, any_epoch, any_protocol_params_update,
            any_protocol_version, any_stake_credential,
        },
        Epoch,
    };
    use num::{BigUint, One};
    use proptest::{collection, option, prelude::*};
    use std::rc::Rc;

    pub fn any_proposal_enum() -> impl Strategy<Value = ProposalEnum> {
        let any_protocol_parameters = (
            option::of(any_comparable_proposal_id()),
            any_protocol_params_update(),
        )
            .prop_map(|(parent, params_update)| {
                ProposalEnum::ProtocolParameters(params_update, parent.map(Rc::new))
            });

        let any_hard_fork = (
            option::of(any_comparable_proposal_id()),
            any_protocol_version(),
        )
            .prop_map(|(parent, protocol_version)| {
                ProposalEnum::HardFork(protocol_version, parent.map(Rc::new))
            });

        let any_constitutional_committee = (
            option::of(any_comparable_proposal_id()),
            any_committee_update(any_epoch()),
        )
            .prop_map(|(parent, committee)| {
                ProposalEnum::ConstitutionalCommittee(committee, parent.map(Rc::new))
            });

        let any_constitution = (option::of(any_comparable_proposal_id()), any_constitution())
            .prop_map(|(parent, constitution)| {
                ProposalEnum::Constitution(constitution, parent.map(Rc::new))
            });

        let any_orphan = any_orphan_proposal().prop_map(ProposalEnum::Orphan);

        prop_oneof![
            any_protocol_parameters,
            any_hard_fork,
            any_constitutional_committee,
            any_constitution,
            any_orphan,
        ]
    }

    pub fn any_orphan_proposal() -> impl Strategy<Value = OrphanProposal> {
        let any_nice_poll = Just(OrphanProposal::NicePoll);

        let any_treasury_withdrawal = collection::btree_map(any_stake_credential(), 1_u64.., 1..3)
            .prop_map(OrphanProposal::TreasuryWithdrawal);

        prop_oneof![any_nice_poll, any_treasury_withdrawal]
    }

    pub fn any_committee_update(
        any_epoch: impl Strategy<Value = Epoch>,
    ) -> impl Strategy<Value = CommitteeUpdate> {
        let any_no_confidence = Just(CommitteeUpdate::NoConfidence);

        let any_change_members = (
            any::<u8>(),
            collection::btree_set(any_stake_credential(), 0..3),
            collection::btree_map(any_stake_credential(), any_epoch, 0..3),
        )
            .prop_map(
                |(numerator, removed, added)| CommitteeUpdate::ChangeMembers {
                    removed,
                    added,
                    threshold: SafeRatio::new(BigUint::from(numerator), BigUint::one()),
                },
            );

        prop_oneof![1 => any_no_confidence, 2 => any_change_members]
    }
}
