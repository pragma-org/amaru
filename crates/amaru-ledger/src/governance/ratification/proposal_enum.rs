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

//       use std::{
//           cmp::Ordering,
//           collections::{BTreeMap, BTreeSet},
//           fmt,
//           rc::Rc,
//       };
//
//       use amaru_kernel::{
//           Constitution, Epoch, GovernanceAction, Lovelace, ProposalId, ProtocolParamUpdate, ProtocolVersion, StakeCredential, rational_number::SafeRatio, into_safe_ratio
//       };
//       // Tests
//       // ----------------------------------------------------------------------------
//       #[cfg(any(test, feature = "test-utils"))]
//       pub use tests::*;
//
//       use crate::summary::{};
//
//       #[cfg(any(test, feature = "test-utils"))]
//       mod tests {
//           use std::rc::Rc;
//
//           use amaru_kernel::{
//               Epoch, any_constitution, any_epoch, any_proposal_id, any_protocol_params_update, any_protocol_version,
//               any_stake_credential,
//           };
//           use num::{BigUint, One};
//           use proptest::{collection, option, prelude::*};
//
//           use super::{CommitteeUpdate, OrphanProposal, ProposalEnum};
//           use crate::summary::SafeRatio;
//
//           pub fn any_proposal_enum() -> impl Strategy<Value = ProposalEnum> {
//               let any_protocol_parameters = (option::of(any_proposal_id()), any_protocol_params_update())
//                   .prop_map(|(parent, params_update)| ProposalEnum::ProtocolParameters(params_update, parent.map(Rc::new)));
//
//               let any_hard_fork = (option::of(any_proposal_id()), any_protocol_version())
//                   .prop_map(|(parent, protocol_version)| ProposalEnum::HardFork(protocol_version, parent.map(Rc::new)));
//
//               let any_constitutional_committee = (option::of(any_proposal_id()), any_committee_update(any_epoch()))
//                   .prop_map(|(parent, committee)| ProposalEnum::ConstitutionalCommittee(committee, parent.map(Rc::new)));
//
//               let any_constitution = (option::of(any_proposal_id()), any_constitution())
//                   .prop_map(|(parent, constitution)| ProposalEnum::Constitution(constitution, parent.map(Rc::new)));
//
//               let any_orphan = any_orphan_proposal().prop_map(ProposalEnum::Orphan);
//
//               prop_oneof![any_protocol_parameters, any_hard_fork, any_constitutional_committee, any_constitution, any_orphan,]
//           }
//
//           pub fn any_orphan_proposal() -> impl Strategy<Value = OrphanProposal> {
//               let any_nice_poll = Just(OrphanProposal::NicePoll);
//
//               let any_treasury_withdrawal = collection::btree_map(any_stake_credential(), 1..(u64::MAX / 3), 1..3)
//                   .prop_map(OrphanProposal::TreasuryWithdrawal);
//
//               prop_oneof![any_nice_poll, any_treasury_withdrawal]
//           }
//
//           pub fn any_committee_update(any_epoch: impl Strategy<Value = Epoch>) -> impl Strategy<Value = CommitteeUpdate> {
//               let any_no_confidence = Just(CommitteeUpdate::NoConfidence);
//
//               let any_change_members = (
//                   any::<u8>(),
//                   collection::btree_set(any_stake_credential(), 0..3),
//                   collection::btree_map(any_stake_credential(), any_epoch, 0..3),
//               )
//                   .prop_map(|(numerator, removed, added)| CommitteeUpdate::ChangeMembers {
//                       removed,
//                       added,
//                       threshold: SafeRatio::new(BigUint::from(numerator), BigUint::one()),
//                   });
//
//               prop_oneof![1 => any_no_confidence, 2 => any_change_members]
//           }
//       }
