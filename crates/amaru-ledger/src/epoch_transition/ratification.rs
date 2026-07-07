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
    AsHash, RatificationStatus, StakeCredentialKind, cost_models, drep_voting_thresholds, ex_units, ex_units_prices,
    pool_voting_thresholds, protocol_version,
};
use amaru_kernel::{
    ComparableProposalId,
    Constitution,
    Epoch,
    EraHistory,
    Lovelace,
    ProtocolParameters,
    StakeCredential,
    cbor,
    // NOTE: We have to import cbor as minicbor here because we derive 'Encode' and 'Decode' traits
    // instances for some types, and the macro rule handling that seems to be explicitly looking
    // for 'minicbor' in scope, and not an alias of any sort...
    cbor as minicbor,
    expect_stake_credential,
    rational_number,
};
use amaru_observability::info_span;
use tracing::{debug, info};

use crate::{
    governance::ratification::{
        CandidateProposal, CommitteeUpdate, ProposalsRoots, ProposalsRootsRc, RatificationContext,
    },
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
    pub pruned_proposals: BTreeMap<ComparableProposalId, RatificationStatus>,

    /// Payouts done to accounts; either because of a deposit refunds or because of a treasury
    /// withdrawal.
    pub payouts: BTreeMap<StakeCredential, Lovelace>,

    /// Captures whether the resulting epoch is considered 'dormant' (i.e. no active proposals
    /// left to vote on at the beginning of the epoch, after ratification).
    pub is_dormant_epoch: bool,

    /// The current constitutional committee, if any. No committee signals a state of
    /// no-confidence.
    pub constitutional_committee: Option<CommitteeUpdate>,

    /// A new constitution that has been voted and approved, if any.
    pub new_constitution: Option<Constitution>,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, cbor::Encode, cbor::Decode, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "camelCase")]
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
        iter_proposals: impl Iterator<Item = (ComparableProposalId, Proposal)>,
        era_history: &EraHistory,
        protocol_parameters: &ProtocolParameters,
        mut ctx: RatificationContext<'_>,
    ) -> Result<Self, StateError> {
        let mut proposals_metadata: BTreeMap<Rc<ComparableProposalId>, ProposalMetadata> = BTreeMap::new();

        // A dual fold where we split the proposal information between 'CandidateProposal' and
        // 'ProposalMetadata'; both used in different contexts.
        let proposals: Vec<(Rc<ComparableProposalId>, CandidateProposal)> = iter_proposals
            .map(|(id, row)| {
                let id = Rc::new(id);

                let candidate = CandidateProposal {
                    valid_until: row.valid_until,
                    proposed_in: row.proposed_in,
                    governance_action: row.proposal.gov_action,
                };

                let metadata = ProposalMetadata {
                    valid_until: row.valid_until,
                    return_account: expect_stake_credential(&row.proposal.reward_account),
                    deposit: row.proposal.deposit,
                };

                proposals_metadata.insert(id.clone(), metadata);

                (id, candidate)
            })
            .collect();

        info_span!(
            ledger::epoch_transition::NEW_GOVERNANCE_UPDATES,
            proposals_count = proposals.len() as u64
        )
        .in_scope(|| {
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
            let mut payouts = ctx.withdrawals;
            let mut payouts_str = String::new();
            for (id, proposal) in proposals_metadata.into_iter() {
                let expired = ctx.epoch == proposal.valid_until;
                let ratified_or_evicted = ctx.pruned_proposals.contains_key(&id);

                debug!(name: "ratification.proposals", proposal_id = %id, expired, ratified_or_evicted);

                if expired || ratified_or_evicted {
                    ctx.pruned_proposals.insert(id, RatificationStatus::NotRatified); // For expired proposals
                    let return_account = proposal.return_account;
                    let deposit = proposal.deposit;
                    payouts
                        .entry(return_account.clone())
                        .and_modify(|balance| {
                            *balance += deposit;
                            trace_return_account(&mut payouts_str, &return_account, *balance);
                        })
                        .or_insert_with(|| {
                            trace_return_account(&mut payouts_str, &return_account, deposit);
                            deposit
                        });
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
            let pruned_proposals: BTreeMap<ComparableProposalId, RatificationStatus> = ctx
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
                name: "ratification.roots",
                constitution = opt_root(roots.constitution.as_deref()),
                constitutional_committee = opt_root(roots.constitutional_committee.as_deref()),
                hard_fork = opt_root(roots.hard_fork.as_deref()),
                protocol_parameters = opt_root(roots.protocol_parameters.as_deref()),
                "ratification.roots",
            );

            info!(
                name: "ratification.miscellaneous",
                pruned_proposals = opt_str(pruned_proposals_str),
                payouts = opt_str(payouts_str),
                new_constitution = opt_str(ctx.new_constitution.as_ref().map(|c| c.anchor.url.clone()).unwrap_or_default()),
                constitutional_committee_update = opt_str(ctx.constitutional_committee_update.as_ref().map(|c| c.to_string()).unwrap_or_default()),
                is_dormant_epoch,
                "ratification.miscellaneous",
            );

            if &ctx.protocol_parameters != protocol_parameters {
                diff_protocol_parameters(protocol_parameters, &ctx.protocol_parameters);
            }

            Ok(Self {
                roots: roots.unwrap_or_clone(),
                pruned_proposals,
                payouts,
                is_dormant_epoch,
                protocol_parameters: ctx.protocol_parameters,
                new_constitution: ctx.new_constitution,
                constitutional_committee: ctx.constitutional_committee_update,
            })
        })
    }

    /// The pending governance payout for the given account, or `0`. This covers both proposal
    /// deposit refunds (on enactment, expiry, or pruning) and treasury withdrawals; both land on
    /// the reward balance at the epoch boundary, so they count towards a withdrawable balance
    /// during the straddle.
    pub fn payout(&self, account: &StakeCredential) -> Lovelace {
        self.payouts.get(account).copied().unwrap_or(0)
    }
}

// ----------------------------------------------------------------------------------------- Tracing

fn trace_return_account(s: &mut String, return_account: &StakeCredential, balance: Lovelace) {
    *s += &format!(
        "{}({}) {}: {}",
        if s.is_empty() { "" } else { ", " },
        StakeCredentialKind::from(return_account),
        return_account.as_hash(),
        balance
    );
}

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
        name: "ratification.new_protocol_parameters",
        protocol_version = opt_field_with(&old.protocol_version, protocol_version, protocol_version::fmt),
        max_block_body_size = opt_field(&old.max_block_body_size, max_block_body_size),
        max_transaction_size = opt_field(&old.max_transaction_size, max_transaction_size),
        max_block_header_size = opt_field(&old.max_block_header_size, max_block_header_size),
        max_tx_ex_units = opt_field_with(&old.max_tx_ex_units, max_tx_ex_units, ex_units::fmt),
        max_block_ex_units = opt_field_with(&old.max_block_ex_units, max_block_ex_units, ex_units::fmt),
        max_value_size = opt_field(&old.max_value_size, max_value_size),
        max_collateral_inputs = opt_field(&old.max_collateral_inputs, max_collateral_inputs),
        min_fee_a = opt_field(&old.min_fee_a, min_fee_a),
        min_fee_b = opt_field(&old.min_fee_b, min_fee_b),
        stake_credential_deposit = opt_field(&old.stake_credential_deposit, stake_credential_deposit),
        stake_pool_deposit = opt_field(&old.stake_pool_deposit, stake_pool_deposit),
        monetary_expansion_rate =
            opt_field_with(&old.monetary_expansion_rate, monetary_expansion_rate, rational_number::fmt),
        treasury_expansion_rate =
            opt_field_with(&old.treasury_expansion_rate, treasury_expansion_rate, rational_number::fmt),
        min_pool_cost = opt_field(&old.min_pool_cost, min_pool_cost),
        lovelace_per_utxo_byte = opt_field(&old.lovelace_per_utxo_byte, lovelace_per_utxo_byte),
        prices = opt_field_with(&old.prices, prices, ex_units_prices::fmt),
        min_fee_ref_script_lovelace_per_byte = opt_field_with(
            &old.min_fee_ref_script_lovelace_per_byte,
            min_fee_ref_script_lovelace_per_byte,
            rational_number::fmt,
        ),
        max_ref_script_size_per_tx =
            opt_field(&old.max_ref_script_size_per_tx, max_ref_script_size_per_tx),
        max_ref_script_size_per_block =
            opt_field(&old.max_ref_script_size_per_block, max_ref_script_size_per_block),
        ref_script_cost_stride = opt_field(&old.ref_script_cost_stride, ref_script_cost_stride),
        ref_script_cost_multiplier =
            opt_field_with(&old.ref_script_cost_multiplier, ref_script_cost_multiplier, rational_number::fmt),
        stake_pool_max_retirement_epoch = opt_field(
            &old.stake_pool_max_retirement_epoch,
            stake_pool_max_retirement_epoch
        ),
        optimal_stake_pools_count =
            opt_field(&old.optimal_stake_pools_count, optimal_stake_pools_count),
        pledge_influence = opt_field_with(&old.pledge_influence, pledge_influence, rational_number::fmt),
        collateral_percentage = opt_field(&old.collateral_percentage, collateral_percentage),
        cost_models = opt_field_with(&old.cost_models, cost_models, cost_models::fmt),
        pool_voting_thresholds = opt_field_with(
            &old.pool_voting_thresholds,
            pool_voting_thresholds,
            pool_voting_thresholds::fmt
        ),
        drep_voting_thresholds = opt_field_with(
            &old.drep_voting_thresholds,
            drep_voting_thresholds,
            drep_voting_thresholds::fmt,
        ),
        min_committee_size = opt_field(&old.min_committee_size, min_committee_size),
        max_committee_term_length =
            opt_field(&old.max_committee_term_length, max_committee_term_length),
        gov_action_lifetime = opt_field(&old.gov_action_lifetime, gov_action_lifetime),
        gov_action_deposit = opt_field(&old.gov_action_deposit, gov_action_deposit),
        drep_deposit = opt_field(&old.drep_deposit, drep_deposit),
        drep_expiry = opt_field(&old.drep_expiry, drep_expiry),
        "ratification.new_protocol_parameters",
    );
}

fn opt_field_with<A: Eq>(old: &A, new: &A, to_string: impl FnOnce(&A) -> String) -> Box<dyn tracing::Value> {
    if old == new { Box::new(tracing::field::Empty) as Box<dyn tracing::Value> } else { Box::new(to_string(new)) }
}

fn opt_field<A: Eq + fmt::Display>(old: &A, new: &A) -> Box<dyn tracing::Value> {
    if old == new { Box::new(tracing::field::Empty) as Box<dyn tracing::Value> } else { Box::new(new.to_string()) }
}

fn opt_str(s: String) -> Box<dyn tracing::Value> {
    if s.is_empty() { Box::new(tracing::field::Empty) as Box<dyn tracing::Value> } else { Box::new(s) }
}

fn opt_root(root: Option<&ComparableProposalId>) -> Box<dyn tracing::Value> {
    root.map(|r| Box::new(r.to_string()) as Box<dyn tracing::Value>).unwrap_or_else(|| Box::new(tracing::field::Empty))
}
