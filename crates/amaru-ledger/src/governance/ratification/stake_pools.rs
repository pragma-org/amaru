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

use super::{CommitteeUpdate, OrphanProposal, ProposalEnum};
use crate::summary::{
    into_safe_ratio, safe_ratio, stake_distribution::StakeDistribution, SafeRatio,
};
use amaru_kernel::{
    expect_stake_credential, protocol_parameters::PoolVotingThresholds, DRep, PoolId,
    ProtocolParamUpdate, ProtocolVersion, Vote, PROTOCOL_VERSION_9,
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
/// - whether a parameter updates contains security-related protocol parameters;
pub fn voting_threshold(
    is_state_of_no_confidence: bool,
    voting_thresholds: &PoolVotingThresholds,
    proposal: &ProposalEnum,
) -> Option<SafeRatio> {
    match proposal {
        ProposalEnum::ProtocolParameters(params_update, _) => {
            if any_update_in_security_group(params_update) {
                Some(into_safe_ratio(
                    &voting_thresholds.security_voting_threshold,
                ))
            } else {
                Some(SafeRatio::zero())
            }
        }

        ProposalEnum::HardFork(..) => {
            Some(into_safe_ratio(&voting_thresholds.hard_fork_initiation))
        }

        ProposalEnum::ConstitutionalCommittee(CommitteeUpdate::NoConfidence, _) => {
            Some(into_safe_ratio(&voting_thresholds.motion_no_confidence))
        }

        ProposalEnum::ConstitutionalCommittee(CommitteeUpdate::ChangeMembers { .. }, _) => {
            Some(if is_state_of_no_confidence {
                into_safe_ratio(&voting_thresholds.committee_no_confidence)
            } else {
                into_safe_ratio(&voting_thresholds.committee_normal)
            })
        }

        ProposalEnum::Constitution(..)
        | ProposalEnum::Orphan(OrphanProposal::TreasuryWithdrawal { .. }) => {
            Some(SafeRatio::zero())
        }

        ProposalEnum::Orphan(OrphanProposal::NicePoll) => None,
    }
}

// Check whether the update contains any parameter that is considered part of the 'security group'.
// Those parameters require approval from the SPO to be changed. Others are only in the hands of
// DReps & Constitutional Committee.
fn any_update_in_security_group(update: &ProtocolParamUpdate) -> bool {
    update.minfee_a.is_some()
        || update.minfee_b.is_some()
        || update.max_block_body_size.is_some()
        || update.max_block_header_size.is_some()
        || update.max_transaction_size.is_some()
        || update.ada_per_utxo_byte.is_some()
        || update.max_block_ex_units.is_some()
        || update.max_value_size.is_some()
        || update.governance_action_deposit.is_some()
        || update.minfee_refscript_cost_per_byte.is_some()
}

// Tally
// ----------------------------------------------------------------------------

/// Count the ratio of yes votes amongst pool operators.
pub fn tally(
    protocol_version: ProtocolVersion,
    proposal: &ProposalEnum,
    votes: BTreeMap<&PoolId, &Vote>,
    stake_distribution: &StakeDistribution,
) -> SafeRatio {
    if stake_distribution.pools_voting_stake == 0 {
        return SafeRatio::zero();
    }

    let (yes, abstain) =
        stake_distribution
            .pools
            .iter()
            .fold((0, 0), |(yes, abstain), (pool_id, pool)| {
                match votes.get(pool_id) {
                    Some(Vote::Yes) => (yes + pool.voting_stake, abstain),
                    Some(Vote::No) => (yes, abstain),
                    Some(Vote::Abstain) => (yes, abstain + pool.voting_stake),

                    // Hard forks always require explicit votes from SPO
                    None if proposal.is_hardfork() => (yes, abstain),

                    // Prior to v10, a pool not voting would be considered abstaining on anything
                    // other than a hard fork.
                    None if protocol_version <= PROTOCOL_VERSION_9 => {
                        (yes, abstain + pool.voting_stake)
                    }

                    // Starting from v10, the fallback is given to the DRep chosen by the pool's
                    // reward account (?!), if any. If there's no drep, then the vote is considered
                    // to be "no" by default.
                    None => {
                        let reward_account =
                            expect_stake_credential(&pool.parameters.reward_account);

                        let drep = stake_distribution
                            .accounts
                            .get(&reward_account)
                            .and_then(|st| st.drep.as_ref());

                        match drep {
                            Some(DRep::NoConfidence) if proposal.is_no_confidence() => {
                                (yes + pool.voting_stake, abstain)
                            }
                            Some(DRep::Abstain) => (yes, abstain + pool.voting_stake),
                            Some(..) | None => (yes, abstain),
                        }
                    }
                }
            });

    let span = tracing::Span::current();
    span.record("votes.pools.yes", yes);
    span.record("votes.pools.abstain", abstain);

    if abstain >= stake_distribution.pools_voting_stake {
        span.record("votes.pools.no", 0);
        SafeRatio::zero()
    } else {
        let no = stake_distribution.pools_voting_stake - abstain;
        span.record("votes.pools.no", no);
        safe_ratio(yes, no)
    }
}

// Tests
// ----------------------------------------------------------------------------

#[cfg(all(test, not(target_os = "windows")))]
mod tests {
    use super::{tally, voting_threshold};
    use crate::{
        governance::ratification::{tests::any_proposal_enum, ProposalEnum},
        summary::{
            stake_distribution::{tests::any_stake_distribution_no_dreps, StakeDistribution},
            SafeRatio,
        },
    };
    use amaru_kernel::{
        expect_stake_credential,
        tests::{
            any_comparable_proposal_id, any_ex_units, any_pool_voting_thresholds,
            any_protocol_params_update, any_protocol_version, any_rational_number,
        },
        DRep, PoolId, ProtocolParamUpdate, ProtocolVersion, Vote, PROTOCOL_VERSION_10,
        PROTOCOL_VERSION_9,
    };
    use num::{One, ToPrimitive, Zero};
    use proptest::{collection, option, prelude::*, sample};
    use std::{collections::BTreeMap, rc::Rc};

    static VOTE_YES: Vote = Vote::Yes;
    static VOTE_NO: Vote = Vote::No;
    static VOTE_ABSTAIN: Vote = Vote::Abstain;

    proptest! {
        #[test]
        fn prop_tally_is_never_greater_than_1((protocol_version, proposal, votes, stake_distribution) in any_tally()) {
            let result = tally(
                protocol_version,
                &proposal,
                votes.iter().map(|(k, v)| (k, *v)).collect(),
                &stake_distribution
            );
            prop_assert!(result <= SafeRatio::one())
        }
    }

    proptest! {
        #[test]
        fn prop_tally_default_differs_between_v9_and_v10((_, proposal, votes, stake_distribution) in any_tally()) {
            let result_v9 = tally(
                PROTOCOL_VERSION_9,
                &proposal,
                votes.iter().map(|(k, v)| (k, *v)).collect(),
                &stake_distribution
            );
            let result_v9_str =
                format!(
                    "{}{}",
                    result_v9,
                    result_v9
                        .to_f64()
                        .map(|s| format!(" ({s})"))
                        .unwrap_or_default()
                );

            let result_v10 = tally(
                PROTOCOL_VERSION_10,
                &proposal,
                votes.iter().map(|(k, v)| (k, *v)).collect(),
                &stake_distribution
            );
            let result_v10_str =
                format!(
                    "{}{}",
                    result_v10,
                    result_v10
                        .to_f64()
                        .map(|s| format!(" ({s})"))
                        .unwrap_or_default()
                );

            let everyone_voted = votes.len() == stake_distribution.pools.len();

            let none_voted_yes = votes.iter().filter(|(_, vote)| **vote == &VOTE_YES).count() == 0;

            let both_zero = result_v9 == SafeRatio::zero() && result_v10 == SafeRatio::zero();

            let non_voters_all_default_abstain =
                    stake_distribution
                        .pools
                        .iter()
                        .filter(|(pool, _st)| !votes.contains_key(pool))
                        .all(|(_pool, st)| {
                            let reward_account = expect_stake_credential(&st.parameters.reward_account);

                            let is_delegated_to_abstain = stake_distribution
                                    .accounts
                                    .get(&reward_account)
                                    .map(|st| st.drep.as_ref() == Some(&DRep::Abstain))
                                    .unwrap_or(false);

                            is_delegated_to_abstain
                        });

            if none_voted_yes {
                prop_assert!(
                    if proposal.is_no_confidence() {
                        result_v9 == SafeRatio::zero()
                    } else {
                        both_zero
                    },
                    "no one voted yes but thresholds are not zero\n\
                    v9 =  {result_v9_str}\n\
                    v10 = {result_v10_str}",
                );
            }

            if everyone_voted || proposal.is_hardfork() || non_voters_all_default_abstain {
                prop_assert!(
                    result_v9 == result_v10,
                    "v9 & v10 should be equal but weren't\n\
                    v9 =  {result_v9_str}\n\
                    v10 = {result_v10_str}\n\
                    everyone_voted? {everyone_voted}\n\
                    non voters all default to abstain? {non_voters_all_default_abstain}\n\
                    is hardfork? {}",
                    proposal.is_hardfork(),
                );
            } else if proposal.is_no_confidence() && none_voted_yes {
                prop_assert!(
                    result_v10 > result_v9 || both_zero,
                    "v10 should be strictly greater than v9, or both null, but they weren't\n\
                    v9 =  {result_v9_str}\n\
                    v10 = {result_v10_str}\n\
                    both zero? {both_zero}"
                );
            } else if !proposal.is_no_confidence() {
                prop_assert!(
                    result_v9 > result_v10 || both_zero,
                    "v9 should be strictly greater than v10 (or both null) but wasn't\n\
                    v9 =  {result_v9_str}\n\
                    v10 = {result_v10_str}\n\
                    both zero? {both_zero}"
                );
            }
        }
    }

    proptest! {
        #[test]
        fn prop_voting_threshold_influenced_by_no_confidence(
            proposal in any_proposal_enum(),
            thresholds in any_pool_voting_thresholds()
        ) {
            let result_normal = voting_threshold(false, &thresholds, &proposal);
            let result_no_confidence = voting_threshold(true, &thresholds, &proposal);

            let identical_thresholds = thresholds.committee_normal == thresholds.committee_no_confidence;

            prop_assert!(
                if proposal.is_committee_member_update() {
                    (result_normal != result_no_confidence) || identical_thresholds
                } else {
                    result_normal == result_no_confidence
                },
                "identical_thresholds? {}\nnormal = {:>7?}\nno-confidence = {:?}",
                identical_thresholds,
                result_normal,
                result_no_confidence,
            )
        }
    }

    proptest! {
        #[test]
        fn prop_voting_threshold_influenced_by_security_params(
            is_no_confidence in any::<bool>(),
            update_in_security_group in any_protocol_params_update_in_security_group(),
            update_no_security_group in any_protocol_params_update_no_security_group(),
            parent in option::of(any_comparable_proposal_id()),
            thresholds in any_pool_voting_thresholds()
        ) {
            let parent = parent.map(Rc::new);

            let proposal_in_security_group = ProposalEnum::ProtocolParameters(update_in_security_group, parent.clone());
            let result_in = voting_threshold(is_no_confidence, &thresholds, &proposal_in_security_group);

            let proposal_no_security_group = ProposalEnum::ProtocolParameters(update_no_security_group, parent.clone());
            let result_no = voting_threshold(is_no_confidence, &thresholds, &proposal_no_security_group);

            let is_null_threshold = thresholds.security_voting_threshold.numerator == 0;

            prop_assert!(
                (result_in > Some(SafeRatio::zero()) || is_null_threshold) && result_no == Some(SafeRatio::zero()),
                "is_null_threshold? {is_null_threshold}\nresult_in: {result_in:?}\nresult_no: {result_no:?}",
            )
        }
    }

    fn any_protocol_params_update_in_security_group() -> impl Strategy<Value = ProtocolParamUpdate>
    {
        let security_group = (
            option::of(any::<u64>()),
            option::of(any::<u64>()),
            option::of(any::<u64>()),
            option::of(any::<u64>()),
            option::of(any::<u64>()),
            option::of(any::<u64>()),
            option::of(any_ex_units()),
            option::of(any::<u64>()),
            option::of(any::<u64>()),
            option::of(any_rational_number()),
        );

        (
            any_protocol_params_update(),
            security_group.prop_filter(
                "not all none",
                |(p0, p1, p2, p3, p4, p5, p6, p7, p8, p9)| {
                    !(p0.is_none()
                        && p1.is_none()
                        && p2.is_none()
                        && p3.is_none()
                        && p4.is_none()
                        && p5.is_none()
                        && p6.is_none()
                        && p7.is_none()
                        && p8.is_none()
                        && p9.is_none())
                },
            ),
        )
            .prop_map(
                |(
                    update,
                    (
                        minfee_a,
                        minfee_b,
                        max_block_body_size,
                        max_transaction_size,
                        max_block_header_size,
                        ada_per_utxo_byte,
                        max_block_ex_units,
                        max_value_size,
                        governance_action_deposit,
                        minfee_refscript_cost_per_byte,
                    ),
                )| ProtocolParamUpdate {
                    minfee_a,
                    minfee_b,
                    max_block_header_size,
                    max_block_body_size,
                    max_transaction_size,
                    ada_per_utxo_byte,
                    max_value_size,
                    max_block_ex_units,
                    governance_action_deposit,
                    minfee_refscript_cost_per_byte,
                    ..update
                },
            )
    }

    fn any_protocol_params_update_no_security_group() -> impl Strategy<Value = ProtocolParamUpdate>
    {
        any_protocol_params_update().prop_map(|update| ProtocolParamUpdate {
            minfee_a: None,
            minfee_b: None,
            max_block_header_size: None,
            max_block_body_size: None,
            max_transaction_size: None,
            ada_per_utxo_byte: None,
            max_value_size: None,
            max_block_ex_units: None,
            governance_action_deposit: None,
            minfee_refscript_cost_per_byte: None,
            ..update
        })
    }

    pub fn any_tally() -> impl Strategy<
        Value = (
            ProtocolVersion,
            ProposalEnum,
            BTreeMap<PoolId, &'static Vote>,
            Rc<StakeDistribution>,
        ),
    > {
        any_stake_distribution_no_dreps().prop_flat_map(|stake_distribution| {
            (
                any_protocol_version(),
                any_proposal_enum(),
                any_votes(&stake_distribution),
                Just(Rc::new(stake_distribution)),
            )
                .prop_map(
                    move |(protocol_version, proposal, votes, stake_distribution)| {
                        (protocol_version, proposal, votes, stake_distribution)
                    },
                )
        })
    }

    pub fn any_vote() -> impl Strategy<Value = &'static Vote> {
        prop_oneof![Just(&VOTE_YES), Just(&VOTE_NO), Just(&VOTE_ABSTAIN)]
    }

    pub fn any_votes(
        stake_distribution: &'_ StakeDistribution,
    ) -> impl Strategy<Value = BTreeMap<PoolId, &'static Vote>> {
        let pools: Vec<PoolId> = stake_distribution.pools.keys().cloned().collect();

        let upper_bound = pools.len() - 1;

        let voters = sample::subsequence(pools, 0..=upper_bound).boxed();

        voters
            .prop_flat_map(|voters| {
                collection::vec(any_vote(), voters.len())
                    .prop_map(move |votes| voters.clone().into_iter().zip(votes))
            })
            .prop_map(|kvs| kvs.into_iter().collect())
    }
}
