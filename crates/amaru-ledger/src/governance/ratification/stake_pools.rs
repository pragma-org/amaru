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
use crate::summary::stake_distribution::StakeDistribution;
use amaru_kernel::{
    expect_stake_credential, protocol_parameters::PoolVotingThresholds, DRep, PoolId,
    ProtocolParamUpdate, ProtocolVersion, RationalNumber, Vote, PROTOCOL_VERSION_9,
};
use num::Rational64;
use std::collections::BTreeMap;

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
) -> Option<Rational64> {
    match proposal {
        ProposalEnum::ProtocolParameters(params_update, _) => {
            if any_update_in_security_group(params_update) {
                Some(into_rational64(
                    &voting_thresholds.security_voting_threshold,
                ))
            } else {
                Some(Rational64::ZERO)
            }
        }
        ProposalEnum::HardFork(..) => {
            Some(into_rational64(&voting_thresholds.hard_fork_initiation))
        }
        ProposalEnum::ConstitutionalCommittee(CommitteeUpdate::NoConfidence, _) => {
            Some(into_rational64(&voting_thresholds.motion_no_confidence))
        }
        ProposalEnum::ConstitutionalCommittee(CommitteeUpdate::ChangeMembers { .. }, _) => {
            Some(if is_state_of_no_confidence {
                into_rational64(&voting_thresholds.committee_no_confidence)
            } else {
                into_rational64(&voting_thresholds.committee_normal)
            })
        }
        ProposalEnum::Constitution(..)
        | ProposalEnum::Orphan(OrphanProposal::TreasuryWithdrawal { .. }) => Some(Rational64::ZERO),
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

/// Count the ratio of yes votes amongst pool operators.
pub fn tally(
    protocol_version: ProtocolVersion,
    proposal: &ProposalEnum,
    votes: BTreeMap<PoolId, &Vote>,
    stake_distribution: &StakeDistribution,
) -> Rational64 {
    if stake_distribution.voting_stake == 0 {
        return Rational64::ZERO;
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

    if abstain >= stake_distribution.voting_stake {
        span.record("votes.pools.no", 0);
        Rational64::ZERO
    } else {
        let no = stake_distribution.voting_stake - abstain;
        span.record("votes.pools.no", no);
        Rational64::new(yes as i64, no as i64)
    }
}

// Helpers
// ----------------------------------------------------------------------------

fn into_rational64(rational: &RationalNumber) -> Rational64 {
    Rational64::new(rational.numerator as i64, rational.denominator as i64)
}

// Tests
// ----------------------------------------------------------------------------

#[cfg(any(test, feature = "test-utils"))]
pub mod tests {
    use crate::summary::{safe_ratio, stake_distribution::StakeDistribution, PoolState};
    use amaru_kernel::{
        tests::{any_pool_id, any_pool_params},
        Epoch, Lovelace, PoolId, Vote,
    };
    use proptest::{collection, prelude::*, prop_compose, sample};
    use std::collections::BTreeMap;

    static VOTE_YES: Vote = Vote::Yes;
    static VOTE_NO: Vote = Vote::No;
    static VOTE_ABSTAIN: Vote = Vote::Abstain;

    pub fn any_votes(
        stake_distribution: &StakeDistribution,
    ) -> impl Strategy<Value = BTreeMap<PoolId, &Vote>> {
        let pools: Vec<PoolId> = stake_distribution.pools.keys().cloned().collect();
        let upper_bound = pools.len() - 1;
        let voters = sample::subsequence(pools, 0..upper_bound);

        voters
            .prop_flat_map(|voters| {
                collection::vec(any_vote(), voters.len())
                    .prop_map(move |votes| voters.clone().into_iter().zip(votes))
            })
            .prop_map(|kvs| kvs.into_iter().collect())
    }

    pub fn any_vote() -> impl Strategy<Value = &'static Vote> {
        prop_oneof![Just(&VOTE_YES), Just(&VOTE_NO), Just(&VOTE_ABSTAIN)]
    }

    prop_compose! {
        pub fn any_pool_state()(
            blocks_count in any::<u64>(),
            stake in any::<Lovelace>(),
            voting_stake in any::<Lovelace>(),
            parameters in any_pool_params(),
        ) -> PoolState {
            let margin = safe_ratio(
                parameters.margin.numerator,
                parameters.margin.denominator,
            );

            PoolState {
                blocks_count,
                stake,
                voting_stake: stake.max(voting_stake),
                margin,
                parameters,
            }
        }
    }

    prop_compose! {
        pub fn any_stake_distribution()(
            epoch in any::<u64>(),
            pools in collection::btree_map(any_pool_id(), any_pool_state(), 1..10),
        ) -> StakeDistribution {
            let voting_stake = pools.values().fold(0, |total, st| total + st.voting_stake);
            let active_stake = pools.values().fold(0, |total, st| total + st.stake);

            StakeDistribution {
                epoch: Epoch::from(epoch),
                active_stake,
                voting_stake,
                pools,
                // TODO: Add some accounts
                accounts: BTreeMap::new(),
                dreps: BTreeMap::new(),
            }
        }
    }
}
