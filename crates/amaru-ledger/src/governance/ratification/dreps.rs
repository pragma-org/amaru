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

use super::{OrphanProposal, ProposalEnum};
use crate::{
    governance::ratification::CommitteeUpdate,
    summary::{SafeRatio, into_safe_ratio, safe_ratio, stake_distribution::StakeDistribution},
};
use amaru_kernel::{
    DRep, DRepVotingThresholds, Epoch, PROTOCOL_VERSION_9, ProtocolParamUpdate, ProtocolVersion,
    Vote,
};
use num::Zero;
use std::collections::BTreeMap;

// Voting Thresholds
// ----------------------------------------------------------------------------

/// Compute the voting threshold corresponding to the proposal; the thresholds are mostly
/// influenced by three things:
///
/// - the kind of proposal;
/// - whether the system is in a state of no-confidence (i.e. is there any lack of constitutional
///   committee?);
/// - whether the system is still in the governance bootstrap phase (protocol version <= 9)
pub fn voting_threshold(
    protocol_version: ProtocolVersion,
    is_state_of_no_confidence: bool,
    voting_thresholds: &DRepVotingThresholds,
    proposal: &ProposalEnum,
) -> Option<SafeRatio> {
    match proposal {
        ProposalEnum::Orphan(OrphanProposal::NicePoll) => None,

        _ if protocol_version <= PROTOCOL_VERSION_9 => Some(SafeRatio::zero()),

        ProposalEnum::ProtocolParameters(update, _) => {
            let network = any_update_in_network_group(voting_thresholds, update);
            let economic = any_update_in_economic_group(voting_thresholds, update);
            let technical = any_update_in_technical_group(voting_thresholds, update);
            let governance = any_update_in_governance_group(voting_thresholds, update);

            Some(
                [network, economic, technical, governance]
                    .into_iter()
                    .flatten()
                    .max()
                    // Technically can't happen / an error, but that's also the behavior on the
                    // Haskell's side.
                    .unwrap_or_else(SafeRatio::zero),
            )
        }

        ProposalEnum::HardFork(..) => {
            Some(into_safe_ratio(&voting_thresholds.hard_fork_initiation))
        }

        ProposalEnum::ConstitutionalCommittee(CommitteeUpdate::NoConfidence, _) => {
            Some(into_safe_ratio(&voting_thresholds.motion_no_confidence))
        }

        ProposalEnum::ConstitutionalCommittee(CommitteeUpdate::ChangeMembers { .. }, _) => {
            Some(into_safe_ratio(if is_state_of_no_confidence {
                &voting_thresholds.committee_no_confidence
            } else {
                &voting_thresholds.committee_normal
            }))
        }

        ProposalEnum::Constitution(..) => {
            Some(into_safe_ratio(&voting_thresholds.update_constitution))
        }

        ProposalEnum::Orphan(OrphanProposal::TreasuryWithdrawal { .. }) => {
            Some(into_safe_ratio(&voting_thresholds.treasury_withdrawal))
        }
    }
}

// Check whether the update contains any parameter that is considered part of the 'network group'.
fn any_update_in_network_group(
    thresholds: &DRepVotingThresholds,
    update: &ProtocolParamUpdate,
) -> Option<SafeRatio> {
    let any = update.max_block_body_size.is_some()
        || update.max_transaction_size.is_some()
        || update.max_block_header_size.is_some()
        || update.max_tx_ex_units.is_some()
        || update.max_block_ex_units.is_some()
        || update.max_value_size.is_some()
        || update.max_collateral_inputs.is_some();

    if any {
        Some(into_safe_ratio(&thresholds.pp_network_group))
    } else {
        None
    }
}

// Check whether the update contains any parameter that is considered part of the 'economic group'.
fn any_update_in_economic_group(
    thresholds: &DRepVotingThresholds,
    update: &ProtocolParamUpdate,
) -> Option<SafeRatio> {
    let any = update.minfee_a.is_some()
        || update.minfee_b.is_some()
        || update.key_deposit.is_some()
        || update.pool_deposit.is_some()
        || update.expansion_rate.is_some()
        || update.treasury_growth_rate.is_some()
        || update.min_pool_cost.is_some()
        || update.ada_per_utxo_byte.is_some()
        || update.execution_costs.is_some()
        || update.minfee_refscript_cost_per_byte.is_some();

    if any {
        Some(into_safe_ratio(&thresholds.pp_economic_group))
    } else {
        None
    }
}

// Check whether the update contains any parameter that is considered part of the 'technical group'.
fn any_update_in_technical_group(
    thresholds: &DRepVotingThresholds,
    update: &ProtocolParamUpdate,
) -> Option<SafeRatio> {
    let any = update.maximum_epoch.is_some()
        || update.desired_number_of_stake_pools.is_some()
        || update.pool_pledge_influence.is_some()
        || update.cost_models_for_script_languages.is_some()
        || update.collateral_percentage.is_some();

    if any {
        Some(into_safe_ratio(&thresholds.pp_technical_group))
    } else {
        None
    }
}

// Check whether the update contains any parameter that is considered part of the 'governance group'.
fn any_update_in_governance_group(
    thresholds: &DRepVotingThresholds,
    update: &ProtocolParamUpdate,
) -> Option<SafeRatio> {
    let any = update.pool_voting_thresholds.is_some()
        || update.drep_voting_thresholds.is_some()
        || update.min_committee_size.is_some()
        || update.committee_term_limit.is_some()
        || update.governance_action_validity_period.is_some()
        || update.governance_action_deposit.is_some()
        || update.drep_deposit.is_some()
        || update.drep_inactivity_period.is_some();

    if any {
        Some(into_safe_ratio(&thresholds.pp_governance_group))
    } else {
        None
    }
}

// Tally
// ----------------------------------------------------------------------------

/// Count the ratio of yes votes amongst dreps.
pub fn tally(
    epoch: Epoch,
    proposal: &ProposalEnum,
    votes: BTreeMap<DRep, &Vote>,
    stake_distribution: &StakeDistribution,
) -> SafeRatio {
    let (yes, denominator) =
        stake_distribution
            .dreps
            .iter()
            .fold((0, 0), |(yes, denominator), (drep, st)| {
                if st.is_active(epoch) {
                    match drep {
                        DRep::Abstain => (yes, denominator),
                        DRep::NoConfidence if proposal.is_no_confidence() => {
                            (yes + st.stake, denominator + st.stake)
                        }
                        DRep::NoConfidence => (yes, denominator + st.stake),
                        DRep::Key(..) | DRep::Script(..) => match votes.get(drep) {
                            None => (yes, denominator + st.stake),
                            Some(Vote::Yes) => (yes + st.stake, denominator + st.stake),
                            Some(Vote::No) => (yes, denominator + st.stake),
                            Some(Vote::Abstain) => (yes, denominator),
                        },
                    }
                } else {
                    (yes, denominator)
                }
            });

    let no = denominator - yes;
    let abstain = stake_distribution.dreps_voting_stake - denominator;

    let span = tracing::Span::current();
    span.record("votes.dreps.yes", yes);
    span.record("votes.dreps.no", no);
    span.record("votes.dreps.abstain", abstain);

    if denominator == 0 {
        SafeRatio::zero()
    } else {
        safe_ratio(yes, denominator)
    }
}

// Tests
// ----------------------------------------------------------------------------

#[cfg(all(test, not(target_os = "windows")))]
mod tests {
    use super::{tally, voting_threshold};
    use crate::{
        governance::ratification::{
            ProposalEnum,
            tests::{MAX_ARBITRARY_EPOCH, MIN_ARBITRARY_EPOCH, any_proposal_enum},
        },
        summary::{
            SafeRatio,
            stake_distribution::{StakeDistribution, tests::any_stake_distribution_no_pools},
        },
    };
    use amaru_kernel::{
        DRep, Epoch, PROTOCOL_VERSION_9, PROTOCOL_VERSION_10, Vote,
        tests::{any_drep_voting_thresholds, any_vote_ref},
    };
    use num::{One, Zero};
    use proptest::{collection, prelude::*, sample, test_runner::RngSeed};
    use std::{collections::BTreeMap, rc::Rc};

    proptest! {
        #[test]
        fn prop_vote_disabled_in_v9(
            is_state_of_no_confidence in any::<bool>(),
            drep_voting_thresholds in any_drep_voting_thresholds(),
            proposal in any_proposal_enum(),
        ) {
            let threshold = voting_threshold(
                PROTOCOL_VERSION_9,
                is_state_of_no_confidence,
                &drep_voting_thresholds,
                &proposal,
            );

            prop_assert!(
                threshold.is_none() && proposal.is_nice_poll()
                || threshold == Some(SafeRatio::zero())
            )
        }
    }

    proptest! {
        #[test]
        fn prop_state_of_no_confidence_only_influence_cc(
            drep_voting_thresholds in any_drep_voting_thresholds(),
            proposal in any_proposal_enum(),
        ) {
            let threshold_normal = voting_threshold(
                PROTOCOL_VERSION_10,
                false,
                &drep_voting_thresholds,
                &proposal,
            );

            let threshold_no_confidence = voting_threshold(
                PROTOCOL_VERSION_10,
                true,
                &drep_voting_thresholds,
                &proposal,
            );

            if proposal.is_committee_member_update() {
                prop_assert!(threshold_normal != threshold_no_confidence)
            } else {
                prop_assert!(threshold_normal == threshold_no_confidence)
            }
        }
    }

    proptest! {
        #[test]
        fn prop_tally_is_never_greater_than_1((epoch, proposal, votes, stake_distribution) in any_tally()) {
            let result = tally(epoch, &proposal, votes, &stake_distribution);
            prop_assert!(result <= SafeRatio::one())
        }
    }

    proptest! {
        #[test]
        fn prop_expired_dreps_do_not_influence_tally((epoch, proposal, votes, stake_distribution) in any_tally()) {
            let result = tally(epoch, &proposal, votes.clone(), &stake_distribution);
            let mut stake_distribution: StakeDistribution = stake_distribution.as_ref().clone();
            stake_distribution.dreps.retain(|_, drep| drep.is_active(epoch));
            let result_no_expired = tally(epoch, &proposal, votes, &stake_distribution);
            prop_assert_eq!(result, result_no_expired)
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig { rng_seed: RngSeed::Fixed(42), ..ProptestConfig::default() })]
        #[test]
        #[should_panic]
        fn prop_generated_dreps_are_sometimes_expired((epoch, _, _, stake_distribution) in any_tally()) {
            let n = stake_distribution.dreps.values().filter(|drep| !drep.is_active(epoch)).count();
            prop_assert_eq!(n, 0, "no expired dreps")
        }
    }

    pub fn any_tally() -> impl Strategy<
        Value = (
            Epoch,
            ProposalEnum,
            BTreeMap<DRep, &'static Vote>,
            Rc<StakeDistribution>,
        ),
    > {
        any_stake_distribution_no_pools(MIN_ARBITRARY_EPOCH, MAX_ARBITRARY_EPOCH).prop_flat_map(
            |stake_distribution| {
                (
                    any_epoch(),
                    any_proposal_enum(),
                    any_votes(&stake_distribution),
                    Just(Rc::new(stake_distribution)),
                )
            },
        )
    }

    pub fn any_votes(
        stake_distribution: &StakeDistribution,
    ) -> impl Strategy<Value = BTreeMap<DRep, &'static Vote>> + use<> {
        let dreps: Vec<DRep> = stake_distribution.dreps.keys().cloned().collect();

        let upper_bound = dreps.len() - 1;

        let voters = sample::subsequence(dreps, 0..=upper_bound).boxed();

        voters
            .prop_flat_map(|voters| {
                collection::vec(any_vote_ref(), voters.len())
                    .prop_map(move |votes| voters.clone().into_iter().zip(votes))
            })
            .prop_map(|kvs| kvs.into_iter().collect())
    }

    pub fn any_epoch() -> impl Strategy<Value = Epoch> {
        (MIN_ARBITRARY_EPOCH..=MAX_ARBITRARY_EPOCH).prop_map(Epoch::from)
    }
}
