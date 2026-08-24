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

use std::{collections::BTreeMap, fmt, rc::Rc};

use amaru_kernel::{
    Constitution,
    ConstitutionalCommitteeUpdate,
    Epoch,
    EraHistory,
    Lovelace,
    // NOTE: We have to import cbor as minicbor here because we derive 'Encode' and 'Decode' traits
    // instances for some types, and the macro rule handling that seems to be explicitly looking
    // for 'minicbor' in scope, and not an alias of any sort...
    ProposalId,
    ProposalsRoots,
    ProposalsRootsRc,
    ProtocolParameters,
    RatificationStatus,
    StakeCredential,
    cbor,
    cbor as minicbor,
    expect_stake_credential,
};
use amaru_observability::{debug, info, info_span};

use crate::{
    governance::ratification::{CandidateProposal, RatificationContext},
    state::StateError,
    store::columns::proposals::Row as Proposal,
};

/// A summary of the governance updates resulting from processing proposals at an epoch boundary.
/// The outcomes are initially stored in this object in-memory before being later flushed to the
/// stable store.
#[derive(Debug)]
pub struct GovernanceUpdates {
    /// Resulting proposal roots for each of the proposal categories.
    pub roots: ProposalsRoots,

    /// Resulting protocol parameters, includes protocol version upgrades for hard forks.
    pub protocol_parameters: ProtocolParameters,

    /// Proposals that have been ratified, have expired or have been pruned due to another
    /// conflicting proposal being dropped.
    pub pruned_proposals: BTreeMap<ProposalId, RatificationStatus>,

    /// Refunds from proposals' deposits that are now being returned due to expiration, enactment or
    /// pruning thereof.
    pub deposit_refunds: BTreeMap<StakeCredential, Lovelace>,

    /// Withdrawals from the treasury by enacted proposals. Kept separate from
    /// `deposit_refunds` which don't come from the treasury at all.
    pub treasury_withdrawals: BTreeMap<StakeCredential, Lovelace>,

    /// Captures whether the resulting epoch is considered 'dormant' (i.e. no active proposals
    /// left to vote on at the beginning of the epoch, after ratification).
    pub is_dormant_epoch: bool,

    /// The current constitutional committee, if any. No committee signals a state of
    /// no-confidence.
    pub constitutional_committee: Option<ConstitutionalCommitteeUpdate>,

    /// A new constitution that has been voted and approved, if any.
    pub new_constitution: Option<Constitution>,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, cbor::Encode, cbor::Decode, serde::Serialize, serde::Deserialize,
)]
pub struct GovernanceActivity {
    #[n(0)]
    pub consecutive_dormant_epochs: u32,
}

/// An intermediate type to capture meta-information related to a particular proposal. This is
/// information common to all proposals.
#[derive(Debug)]
struct ProposalMetadata {
    valid_until: Epoch,
    return_account: StakeCredential,
    deposit: Lovelace,
}

impl GovernanceUpdates {
    #[cfg(any(test, feature = "test-utils"))]
    pub fn default(protocol_parameters: ProtocolParameters) -> Self {
        Self {
            roots: ProposalsRoots::default(),
            protocol_parameters,
            pruned_proposals: BTreeMap::default(),
            deposit_refunds: BTreeMap::default(),
            treasury_withdrawals: BTreeMap::default(),
            is_dormant_epoch: true,
            constitutional_committee: None,
            new_constitution: None,
        }
    }

    /// Look at every still-active governance proposal and ratify them in order of priority and
    /// submission.
    ///
    /// This generates a few outcomes:
    ///
    /// 1. There are some governance consequences such as withdrawals, protocol parameters changes,
    ///    etc...
    ///
    /// 2. Proposals that:
    ///
    ///     - are ratified
    ///     - are dropped due to other conflicting proposals
    ///     - expire
    ///
    ///    Will trigger a refund of their original deposit amount to the registered credential.
    ///    Their corresponding votes can also be pruned from the database.
    ///
    /// 3. The 'governance activity' must be updated accordingly; it captures the number of
    ///    consecutive dormant epochs, which is used to figure out DReps inactivity (DReps
    ///    aren't penalized for not being active in epochs where there's no activity).
    ///
    pub fn new(
        roots: ProposalsRootsRc,
        iter_proposals: impl Iterator<Item = (ProposalId, Proposal)>,
        era_history: &EraHistory,
        protocol_parameters: &ProtocolParameters,
        mut ctx: RatificationContext<'_>,
    ) -> Result<Self, StateError> {
        let mut proposals_metadata: BTreeMap<Rc<ProposalId>, ProposalMetadata> = BTreeMap::new();

        // A dual fold where we split the proposal information between 'CandidateProposal' and
        // 'ProposalMetadata'; both used in different contexts.
        let proposals: Vec<(Rc<ProposalId>, CandidateProposal)> = iter_proposals
            .map(|(id, row)| {
                let id = Rc::new(id);

                let metadata = ProposalMetadata {
                    valid_until: row.valid_until,
                    return_account: expect_stake_credential(&row.proposal.reward_account),
                    deposit: row.proposal.deposit,
                };

                proposals_metadata.insert(id.clone(), metadata);

                (id, CandidateProposal::from(row))
            })
            .collect();

        info_span!(ledger::epoch_transition::NEW_GOVERNANCE_UPDATES, proposals_count = proposals.len() as u64).in_scope(
            || {
                let roots = ctx
                    .ratify_proposals(
                        era_history,
                        // Get all proposals to ratify / enact. Note that, even though the ratification happens
                        // with an epoch of delay (and thus, using data from a snapshot), we always use the most
                        // recent set of proposals available. While recently submitted proposals won't have any
                        // votes, they might still end up being pruned due to a previous proposal being enacted.
                        //
                        // TODO: Lazily fetch governance proposals on epoch boundary
                        //
                        // We shouldn't collect all proposals here, but provides iterators for the
                        // ratification step to go over them lazily.
                        proposals,
                        roots,
                    )
                    .map_err(|e| StateError::RatificationFailed(e.to_string()))?;

                // Once ratified, we can go over each proposal and figure out refunds due to
                // enactment, expiry or conflicts with other enacted proposals.
                let mut is_dormant_epoch = true;
                let mut deposit_refunds = BTreeMap::new();
                for (id, proposal) in proposals_metadata.into_iter() {
                    let expired = ctx.epoch == proposal.valid_until;
                    let ratified_or_evicted = ctx.pruned_proposals.contains_key(&id);

                    if expired || ratified_or_evicted {
                        info!(ledger::proposal::DROP, id = id.to_string(), expired, ratified_or_evicted);
                        // Expired proposals aren't in the pruned set yet; ratified or evicted ones
                        // already are, and must keep the status recorded during ratification.
                        ctx.pruned_proposals.entry(id).or_insert(RatificationStatus::NotRatified);
                        let return_account = proposal.return_account;
                        let deposit = proposal.deposit;
                        deposit_refunds
                            .entry(return_account)
                            .and_modify(|balance| {
                                *balance += deposit;
                            })
                            .or_insert_with(|| deposit);
                    } else {
                        // NOTE: dormant epochs
                        //
                        // An epoch is said to be 'dormant' if there's no active proposals at the beginning of
                        // the epoch, after ratification has occured. However, since proposals are ratified
                        // with an epoch of delay, the `ctx.epoch` refers to 2 epochs in the past compare
                        // the one that is just starting.
                        //
                        // Consider the following diagram, with a proposal valid until epoch e+1; with
                        // no other proposals. The proposal expires in the transition from e+1 to e+2, so
                        // that e+2 shall be considered dormant.
                        //
                        //                 │ ratifying       │ ratifying       │ ratifying
                        //                 │ for e - 2       │ for e - 1       │ for e
                        //                 │                 │                 │
                        //                 ╽                 ╽                 ╽
                        // ━━━━━━━━━━━━╸╸╸╋╸╸╸╸━██━██━██━╸╸╸╋╸╸╸╸━██━██━██━╸╸╸╋╸╸╸╸━██━██━██━╸╸╸━━━>
                        //      e - 1              e               e + 1             e + 2
                        //
                        is_dormant_epoch = is_dormant_epoch && proposal.valid_until < ctx.epoch + 2;
                    }
                }

                // NOTE: 'unwrap_or_clone' pruned proposal ids
                //
                // We have disposed of the proposals metadata just before by consuming the object via
                // 'into_iter'. This object should constitutes the last remaining Rc counts for the
                // proposal ids, so that the next 'unwrap_or_clone' should in practice results in a
                // clean transfer of ownership without clone.
                let mut pruned_proposals_str = String::new();
                let pruned_proposals: BTreeMap<ProposalId, RatificationStatus> = ctx
                    .pruned_proposals
                    .into_iter()
                    .map(|(id, status)| {
                        let id = Rc::unwrap_or_clone(id);

                        if pruned_proposals_str.is_empty() {
                            pruned_proposals_str = id.to_string();
                        } else {
                            pruned_proposals_str += &format!(", {id}");
                        }

                        (id, status)
                    })
                    .collect();

                debug!(
                    ledger::proposal_roots::SUMMARIZE,
                    constitution = @opt_root(roots.constitution.as_deref()),
                    constitutional_committee = @opt_root(roots.constitutional_committee.as_deref()),
                    hard_fork = @opt_root(roots.hard_fork.as_deref()),
                    protocol_parameters = @opt_root(roots.protocol_parameters.as_deref()),
                );

                info!(
                    ledger::ratification::SUMMARIZE,
                    pruned_proposals = @opt_str(pruned_proposals_str),
                    refunds = @opt_map(&deposit_refunds),
                    withdrawals = @opt_map(&ctx.withdrawals),
                    new_constitution =
                        @opt_str(ctx.new_constitution.as_ref().map(|c| (*c.anchor.url).to_string()).unwrap_or_default()),
                    constitutional_committee_update = @opt_str(
                        ctx.constitutional_committee_update.as_ref().map(|c| c.to_string()).unwrap_or_default()
                    ),
                    is_dormant_epoch,
                );

                if &ctx.protocol_parameters != protocol_parameters {
                    diff_protocol_parameters(protocol_parameters, &ctx.protocol_parameters);
                }

                Ok(Self {
                    roots: roots.unwrap_or_clone(),
                    pruned_proposals,
                    deposit_refunds,
                    treasury_withdrawals: ctx.withdrawals,
                    protocol_parameters: ctx.protocol_parameters,
                    new_constitution: ctx.new_constitution,
                    constitutional_committee: ctx.constitutional_committee_update,
                    is_dormant_epoch,
                })
            },
        )
    }

    /// The pending governance payout for the given account, or `0`. This covers both proposal
    /// deposit refunds (on enactment, expiry, or pruning) and treasury withdrawals; both land on
    /// the reward balance at the epoch boundary, so they count towards a withdrawable balance
    /// during the straddle.
    pub fn payout(&self, account: &StakeCredential) -> Lovelace {
        let refund = self.deposit_refunds.get(account).copied().unwrap_or(0);
        let withdrawal = self.treasury_withdrawals.get(account).copied().unwrap_or(0);

        refund + withdrawal
    }
}

// ----------------------------------------------------------------------------------------- Tracing

fn diff_protocol_parameters(old: &ProtocolParameters, new: &ProtocolParameters) {
    // NOTE: destructuring for completeness static checks
    let ProtocolParameters {
        protocol_version,
        max_block_body_size,
        max_transaction_size,
        max_block_header_size,
        max_tx_ex_units,
        max_block_ex_units,
        max_value_size,
        max_collateral_inputs,
        min_fee_a,
        min_fee_b,
        stake_credential_deposit,
        stake_pool_deposit,
        monetary_expansion_rate,
        treasury_expansion_rate,
        min_pool_cost,
        lovelace_per_utxo_byte,
        prices,
        min_fee_ref_script_lovelace_per_byte,
        max_ref_script_size_per_tx,
        max_ref_script_size_per_block,
        ref_script_cost_stride,
        ref_script_cost_multiplier,
        stake_pool_max_retirement_epoch,
        optimal_stake_pools_count,
        pledge_influence,
        collateral_percentage,
        cost_models,
        pool_voting_thresholds,
        drep_voting_thresholds,
        min_committee_size,
        max_committee_term_length,
        gov_action_lifetime,
        gov_action_deposit,
        drep_deposit,
        drep_expiry,
    } = new;

    info!(
        ledger::protocol_parameters::RATIFY,
        protocol_version = @opt_field(&old.protocol_version, protocol_version),
        max_block_body_size = @opt_field(&old.max_block_body_size, max_block_body_size),
        max_transaction_size = @opt_field(&old.max_transaction_size, max_transaction_size),
        max_block_header_size = @opt_field(&old.max_block_header_size, max_block_header_size),
        max_tx_ex_units = @opt_field(&old.max_tx_ex_units, max_tx_ex_units),
        max_block_ex_units = @opt_field(&old.max_block_ex_units, max_block_ex_units),
        max_value_size = @opt_field(&old.max_value_size, max_value_size),
        max_collateral_inputs = @opt_field(&old.max_collateral_inputs, max_collateral_inputs),
        min_fee_a = @opt_field(&old.min_fee_a, min_fee_a),
        min_fee_b = @opt_field(&old.min_fee_b, min_fee_b),
        stake_credential_deposit = @opt_field(&old.stake_credential_deposit, stake_credential_deposit),
        stake_pool_deposit = @opt_field(&old.stake_pool_deposit, stake_pool_deposit),
        monetary_expansion_rate =
            @opt_field(&old.monetary_expansion_rate, monetary_expansion_rate),
        treasury_expansion_rate =
            @opt_field(&old.treasury_expansion_rate, treasury_expansion_rate),
        min_pool_cost = @opt_field(&old.min_pool_cost, min_pool_cost),
        lovelace_per_utxo_byte = @opt_field(&old.lovelace_per_utxo_byte, lovelace_per_utxo_byte),
        prices = @opt_field(&old.prices, prices),
        min_fee_ref_script_lovelace_per_byte = @opt_field(
            &old.min_fee_ref_script_lovelace_per_byte,
            min_fee_ref_script_lovelace_per_byte,
        ),
        max_ref_script_size_per_tx = @opt_field(&old.max_ref_script_size_per_tx, max_ref_script_size_per_tx),
        max_ref_script_size_per_block = @opt_field(&old.max_ref_script_size_per_block, max_ref_script_size_per_block),
        ref_script_cost_stride = @opt_field(&old.ref_script_cost_stride, ref_script_cost_stride),
        ref_script_cost_multiplier =
            @opt_field(&old.ref_script_cost_multiplier, ref_script_cost_multiplier),
        stake_pool_max_retirement_epoch =
            @opt_field(&old.stake_pool_max_retirement_epoch, stake_pool_max_retirement_epoch),
        optimal_stake_pools_count = @opt_field(&old.optimal_stake_pools_count, optimal_stake_pools_count),
        pledge_influence = @opt_field(&old.pledge_influence, pledge_influence),
        collateral_percentage = @opt_field(&old.collateral_percentage, collateral_percentage),
        cost_models = @opt_field(&old.cost_models, cost_models),
        pool_voting_thresholds = @opt_field(&old.pool_voting_thresholds, pool_voting_thresholds),
        drep_voting_thresholds = @opt_field(&old.drep_voting_thresholds, drep_voting_thresholds),
        min_committee_size = @opt_field(&old.min_committee_size, min_committee_size),
        max_committee_term_length = @opt_field(&old.max_committee_term_length, max_committee_term_length),
        gov_action_lifetime = @opt_field(&old.gov_action_lifetime, gov_action_lifetime),
        gov_action_deposit = @opt_field(&old.gov_action_deposit, gov_action_deposit),
        drep_deposit = @opt_field(&old.drep_deposit, drep_deposit),
        drep_expiry = @opt_field(&old.drep_expiry, drep_expiry),
    );
}

fn opt_field<A: Eq + fmt::Display>(old: &A, new: &A) -> Box<dyn tracing::Value> {
    if old == new { Box::new(tracing::field::Empty) as Box<dyn tracing::Value> } else { Box::new(new.to_string()) }
}

fn opt_str(s: String) -> Box<dyn tracing::Value> {
    if s.is_empty() { Box::new(tracing::field::Empty) as Box<dyn tracing::Value> } else { Box::new(s) }
}

fn opt_map<K: fmt::Display, V: fmt::Display>(map: &BTreeMap<K, V>) -> Box<dyn tracing::Value> {
    let mut s = String::new();
    for (k, v) in map {
        s += &format!("{}{k}={v}", if s.is_empty() { "" } else { ", " });
    }
    opt_str(s)
}

fn opt_root(root: Option<&ProposalId>) -> Box<dyn tracing::Value> {
    root.map(|r| Box::new(r.to_string()) as Box<dyn tracing::Value>).unwrap_or_else(|| Box::new(tracing::field::Empty))
}

#[cfg(test)]
mod tests {
    use std::{collections::VecDeque, sync::Mutex};

    use amaru_kernel::{GovernanceAction, PREPROD_DEFAULT_PROTOCOL_PARAMETERS, PREPROD_ERA_HISTORY, any_proposal_id};
    use proptest::{prelude::Strategy, strategy::ValueTree, test_runner::TestRunner};

    use super::*;
    use crate::{
        state::StakeDistributionView, store::columns::proposals, summary::stake_distribution::StakeDistribution,
    };

    fn empty_stake_distribution(epoch: Epoch) -> StakeDistribution {
        StakeDistribution {
            epoch,
            treasury: 0,
            reserves: 0,
            active_stake: 0,
            pools_voting_stake: 0,
            dreps_voting_stake: 0,
            pools: BTreeMap::new(),
            dreps: BTreeMap::new(),
        }
    }

    fn any_information_proposal(runner: &mut TestRunner, valid_until: Epoch) -> Proposal {
        let mut row = proposals::tests::any_row(1_000).new_tree(runner).unwrap().current();
        row.valid_until = valid_until;
        row.proposal.deposit = 100_000;
        row.proposal.gov_action = GovernanceAction::Information;
        row
    }

    #[test]
    fn dropped_proposals_keep_their_ratification_status() {
        let mut runner = TestRunner::default();
        let epoch = Epoch::from(10);

        let ratified_id = any_proposal_id().new_tree(&mut runner).unwrap().current();
        let expired_id = any_proposal_id().new_tree(&mut runner).unwrap().current();

        let ratified = any_information_proposal(&mut runner, epoch + 5);
        let expired = any_information_proposal(&mut runner, epoch);

        let withdrawal_account = expect_stake_credential(&ratified.proposal.reward_account);

        let distributions = Mutex::new(VecDeque::from([empty_stake_distribution(epoch)]));
        let ctx = RatificationContext {
            epoch,
            treasury: 1_000_000_000,
            stake_distribution: StakeDistributionView::new(distributions.lock().unwrap(), epoch).unwrap(),
            protocol_parameters: PREPROD_DEFAULT_PROTOCOL_PARAMETERS.clone(),
            pruned_proposals: BTreeMap::from([(Rc::new(ratified_id), RatificationStatus::Ratified)]),
            withdrawals: BTreeMap::from([(withdrawal_account, 70_000)]),
            constitutional_committee: None,
            constitutional_committee_update: None,
            new_constitution: None,
            votes: BTreeMap::new(),
        };

        let updates = GovernanceUpdates::new(
            ProposalsRootsRc::default(),
            [(ratified_id, ratified), (expired_id, expired)].into_iter(),
            &PREPROD_ERA_HISTORY,
            &PREPROD_DEFAULT_PROTOCOL_PARAMETERS,
            ctx,
        )
        .unwrap();

        assert_eq!(
            updates.pruned_proposals.get(&ratified_id),
            Some(&RatificationStatus::Ratified),
            "a proposal pruned during ratification must keep its 'Ratified' status"
        );
        assert_eq!(
            updates.pruned_proposals.get(&expired_id),
            Some(&RatificationStatus::NotRatified),
            "an expired proposal is 'NotRatified'"
        );
        assert_eq!(
            updates.treasury_withdrawals.values().sum::<Lovelace>(),
            70_000,
            "enacted withdrawals are totalled for the treasury debit"
        );
    }
}
