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

use amaru_kernel::{
    Ballot, BallotId, BlockHeight, ComparableProposalId, DRep, DRepRegistration, GovernanceAction, Hash,
    MemoizedTransactionOutput, Point, PoolId, PoolParams, Proposal, ProposalsRoots, Slot, StakeCredential, Tip,
    TransactionInput, any_anchor, any_ballot, any_ballot_id, any_comparable_proposal_id, any_drep,
    any_drep_registration, any_modern_output, any_pool_params, any_proposals_roots, any_reward_account,
    any_stake_credential,
    utils::tests::{random_bytes_with_rng, run_strategy_with_rng},
};
use rand::Rng;

// -------------------------------------------------------------------------------------- Generators

pub fn ballot(rng: &mut impl Rng) -> Ballot {
    run_strategy_with_rng(rng, any_ballot())
}

pub fn ballot_id(rng: &mut impl Rng) -> BallotId {
    run_strategy_with_rng(rng, any_ballot_id())
}

pub fn comparable_proposal_id(rng: &mut impl Rng) -> ComparableProposalId {
    run_strategy_with_rng(rng, any_comparable_proposal_id())
}

pub fn drep(rng: &mut impl Rng) -> DRep {
    run_strategy_with_rng(rng, any_drep())
}

pub fn drep_registration(rng: &mut impl Rng) -> DRepRegistration {
    run_strategy_with_rng(rng, any_drep_registration())
}

pub fn hash28(rng: &mut impl Rng) -> Hash<28> {
    Hash::from(random_bytes_with_rng(rng, 28).as_slice())
}

pub fn hash32(rng: &mut impl Rng) -> Hash<32> {
    Hash::from(random_bytes_with_rng(rng, 32).as_slice())
}

pub fn input(rng: &mut impl Rng) -> TransactionInput {
    TransactionInput { transaction_id: hash32(rng), index: rng.random() }
}

pub fn output(rng: &mut impl Rng) -> MemoizedTransactionOutput {
    run_strategy_with_rng(rng, any_modern_output())
}

pub fn pool_id(rng: &mut impl Rng) -> PoolId {
    hash28(rng)
}

pub fn pool_params(rng: &mut impl Rng) -> PoolParams {
    run_strategy_with_rng(rng, any_pool_params())
}

pub fn point(rng: &mut impl Rng, ix: u64) -> Point {
    let slot = Slot::from(ix + 1);
    let hash = hash32(rng);
    Point::Specific(slot, hash)
}

pub fn proposal(rng: &mut impl Rng) -> Proposal {
    Proposal {
        deposit: rng.random(),
        reward_account: run_strategy_with_rng(rng, any_reward_account()),
        gov_action: GovernanceAction::Information,
        anchor: run_strategy_with_rng(rng, any_anchor()),
    }
}

pub fn proposals_roots(rng: &mut impl Rng) -> ProposalsRoots {
    run_strategy_with_rng(rng, any_proposals_roots())
}

pub fn stake_credential(rng: &mut impl Rng) -> StakeCredential {
    run_strategy_with_rng(rng, any_stake_credential())
}

pub fn tip(rng: &mut impl Rng, ix: u64) -> Tip {
    let height = BlockHeight::from(ix + 1);
    Tip::new(point(rng, ix), height)
}

// ---------------------------------------------------------------------------------------- Defaults

pub fn default_pool_id() -> PoolId {
    PoolId::from([0; 28])
}
