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

use std::collections::BTreeMap;

use amaru_kernel::{
    ConstitutionalCommitteeUpdate, DRep, OrphanProposal, PoolId, PoolVotingThresholds, ProposalEnum, Vote,
    rational_number::{SafeRatio, into_safe_ratio, safe_ratio},
};
use num::Zero;

use crate::summary::stake_distribution::StakeDistribution;

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
            if params_update.any_in_security_group() {
                Some(into_safe_ratio(&voting_thresholds.security_voting_threshold))
            } else {
                Some(SafeRatio::zero())
            }
        }

        ProposalEnum::HardFork(..) => Some(into_safe_ratio(&voting_thresholds.hard_fork_initiation)),

        ProposalEnum::ConstitutionalCommittee(ConstitutionalCommitteeUpdate::NoConfidence, _) => {
            Some(into_safe_ratio(&voting_thresholds.motion_no_confidence))
        }

        ProposalEnum::ConstitutionalCommittee(ConstitutionalCommitteeUpdate::ChangeMembers { .. }, _) => {
            Some(if is_state_of_no_confidence {
                into_safe_ratio(&voting_thresholds.committee_no_confidence)
            } else {
                into_safe_ratio(&voting_thresholds.committee_normal)
            })
        }

        ProposalEnum::Constitution(..) | ProposalEnum::Orphan(OrphanProposal::TreasuryWithdrawal { .. }) => {
            Some(SafeRatio::zero())
        }

        ProposalEnum::Orphan(OrphanProposal::NicePoll) => None,
    }
}

// Tally
// ----------------------------------------------------------------------------

/// Count the ratio of yes votes amongst pool operators.
pub fn tally(
    proposal: &ProposalEnum,
    votes: BTreeMap<&PoolId, &Vote>,
    stake_distribution: &StakeDistribution,
) -> SafeRatio {
    if stake_distribution.pools_voting_stake == 0 {
        return SafeRatio::zero();
    }

    let (yes, abstain) = stake_distribution.pools.iter().fold((0, 0), |(yes, abstain), (pool_id, pool)| {
        match votes.get(pool_id) {
            Some(Vote::Yes) => (yes + pool.voting_stake, abstain),
            Some(Vote::No) => (yes, abstain),
            Some(Vote::Abstain) => (yes, abstain + pool.voting_stake),

            // Hard forks always require explicit votes from SPO
            None if proposal.is_hardfork() => (yes, abstain),
            // Starting from v10, the fallback is given to the DRep chosen by the pool's
            // reward account (?!), if any. If there's no drep, then the vote is considered
            // to be "no" by default.
            None => match pool.fallback_drep.as_ref() {
                Some(DRep::NoConfidence) if proposal.is_no_confidence() => (yes + pool.voting_stake, abstain),
                Some(DRep::Abstain) => (yes, abstain + pool.voting_stake),
                Some(..) | None => (yes, abstain),
            },
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
    use std::{collections::BTreeMap, rc::Rc};

    use amaru_kernel::{
        CertificatePointer, ConstitutionalCommitteeUpdate, Credential, DRep, Hash, Network, PoolId, PoolParams,
        ProposalEnum, ProtocolParamUpdate, RationalNumber, RewardAccount, SafeRatio, Vote, any_ex_units,
        any_pool_voting_thresholds, any_proposal_enum, any_proposal_id, any_protocol_params_update,
        any_rational_number, any_vote_ref, safe_ratio,
    };
    use num::{One, Zero};
    use proptest::{collection, option, prelude::*, sample};

    use super::{tally, voting_threshold};
    use crate::summary::{
        PoolState,
        stake_distribution::{StakeDistribution, tests::any_stake_distribution_no_dreps},
    };

    proptest! {
        #[test]
        fn prop_tally_is_never_greater_than_1((proposal, votes, stake_distribution) in any_tally()) {
            let result = tally(
                &proposal,
                votes.iter().map(|(k, v)| (k, *v)).collect(),
                &stake_distribution
            );
            prop_assert!(result <= SafeRatio::one())
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
            parent in option::of(any_proposal_id()),
            thresholds in any_pool_voting_thresholds()
        ) {
            let parent = parent.map(Rc::new);

            let proposal_in_security_group = ProposalEnum::ProtocolParameters(Box::new(update_in_security_group), parent.clone());
            let result_in = voting_threshold(is_no_confidence, &thresholds, &proposal_in_security_group);

            let proposal_no_security_group = ProposalEnum::ProtocolParameters(Box::new(update_no_security_group), parent.clone());
            let result_no = voting_threshold(is_no_confidence, &thresholds, &proposal_no_security_group);

            let is_null_threshold = thresholds.security_voting_threshold.numerator == 0;

            prop_assert!(
                (result_in > Some(SafeRatio::zero()) || is_null_threshold) && result_no == Some(SafeRatio::zero()),
                "is_null_threshold? {is_null_threshold}\nresult_in: {result_in:?}\nresult_no: {result_no:?}",
            )
        }
    }

    #[test]
    fn tally_uses_pool_fallback_drep_when_operator_is_silent() {
        let pool_id = PoolId::new([1; 28]);

        let tally = tally(
            &ProposalEnum::ConstitutionalCommittee(ConstitutionalCommitteeUpdate::NoConfidence, None),
            BTreeMap::new(),
            &stake_summary_with_fallback(pool_id, Some(DRep::NoConfidence)),
        );

        assert_eq!(tally, SafeRatio::one());
    }

    #[test]
    fn tally_treats_abstaining_fallback_drep_as_abstain() {
        let pool_id = PoolId::new([1; 28]);

        let tally = tally(
            &ProposalEnum::ConstitutionalCommittee(ConstitutionalCommitteeUpdate::NoConfidence, None),
            BTreeMap::new(),
            &stake_summary_with_fallback(pool_id, Some(DRep::Abstain)),
        );

        assert_eq!(tally, SafeRatio::zero());
    }

    fn any_protocol_params_update_in_security_group() -> impl Strategy<Value = ProtocolParamUpdate> {
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
            security_group.prop_filter("not all none", |(p0, p1, p2, p3, p4, p5, p6, p7, p8, p9)| {
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
            }),
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

    fn any_protocol_params_update_no_security_group() -> impl Strategy<Value = ProtocolParamUpdate> {
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

    pub fn any_tally() -> impl Strategy<Value = (ProposalEnum, BTreeMap<PoolId, &'static Vote>, Rc<StakeDistribution>)>
    {
        any_stake_distribution_no_dreps().prop_flat_map(|stake_distribution| {
            (any_proposal_enum(), any_votes(&stake_distribution), Just(Rc::new(stake_distribution)))
                .prop_map(move |(proposal, votes, stake_distribution)| (proposal, votes, stake_distribution))
        })
    }

    pub fn any_votes(
        stake_distribution: &StakeDistribution,
    ) -> impl Strategy<Value = BTreeMap<PoolId, &'static Vote>> + use<> {
        let pools: Vec<PoolId> = stake_distribution.pools.keys().cloned().collect();

        let upper_bound = pools.len() - 1;

        let voters = sample::subsequence(pools, 0..=upper_bound).boxed();

        voters
            .prop_flat_map(|voters| {
                collection::vec(any_vote_ref(), voters.len())
                    .prop_map(move |votes| voters.clone().into_iter().zip(votes))
            })
            .prop_map(|kvs| kvs.into_iter().collect())
    }

    fn stake_summary_with_fallback(pool_id: PoolId, fallback_drep: Option<DRep>) -> StakeDistribution {
        StakeDistribution {
            epoch: 0.into(),
            treasury: 0,
            reserves: 0,
            active_stake: 100,
            pools_voting_stake: 100,
            dreps_voting_stake: 0,
            pools: BTreeMap::from([(
                pool_id,
                PoolState {
                    registered_at: CertificatePointer::default(),
                    blocks_count: 0,
                    stake: 100,
                    voting_stake: 100,
                    margin: safe_ratio(0, 1),
                    parameters: PoolParams {
                        id: pool_id,
                        vrf: Hash::new([7; 32]),
                        pledge: 0,
                        cost: 0,
                        margin: RationalNumber { numerator: 0, denominator: 1 },
                        reward_account: RewardAccount::new(
                            Network::Testnet,
                            Credential::ScriptHash(Hash::new([1; 28])),
                        ),
                        owners: Vec::new(),
                        relays: Vec::new(),
                        metadata: None,
                    },
                    fallback_drep,
                },
            )]),
            dreps: BTreeMap::new(),
        }
    }
}
