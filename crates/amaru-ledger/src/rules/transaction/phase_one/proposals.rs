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

use std::collections::BTreeSet;

use amaru_kernel::{
    Address, Epoch, EraHistory, GovernanceAction, Hash, Lovelace, MemoizedDatum, Network, Proposal, ProposalId,
    ProposalPointer, ProposalSlim, ProtocolParamUpdate, ProtocolParameters, ProtocolVersion, RedeemerTag,
    RequiredScript, StakeCredential, TransactionId, TransactionPointer, size::SCRIPT,
};
use thiserror::Error;

use crate::context::{AccountsSlice, BalanceSlice, ProposalsSlice, WitnessSlice};

#[derive(Debug, Error)]
pub enum InvalidProposals {
    #[error("incorrect proposal deposit: provided {provided}, expected {expected}")]
    IncorrectDeposit { provided: Lovelace, expected: Lovelace },

    #[error("proposal return address has wrong network: expected {expected:?}, actual {actual:?}")]
    ReturnAddressWrongNetwork { expected: Network, actual: Network },

    #[error("proposal return address is malformed")]
    MalformedReturnAddress,

    #[error("proposal return account does not exist: {0:?}")]
    ProposalReturnAccountDoesNotExist(StakeCredential),

    #[error("treasury withdrawals total is zero")]
    TreasuryWithdrawalsAllZeros,

    #[error("treasury withdrawal address has wrong network: expected {expected:?}, actual {actual:?}")]
    TreasuryWithdrawalWrongNetwork { expected: Network, actual: Network },

    #[error("treasury withdrawal return accounts do not exist: {0:?}")]
    TreasuryWithdrawalReturnAccountsDoNotExist(BTreeSet<StakeCredential>),

    #[error("conflicting committee update: members appear in both add and remove sets")]
    ConflictingCommitteeUpdate,

    #[error(
        "hardfork version {new_version:?} cannot follow version {last_proposed_version:?} while current version is {current_version:?}"
    )]
    HardforkCantFollow {
        last_proposed_version: ProtocolVersion,
        new_version: ProtocolVersion,
        current_version: ProtocolVersion,
    },

    #[error("malformed parameter change proposal: {reason}")]
    MalformedProposal { reason: String },

    #[error("committee member expiration epoch {expiry} is not greater than current epoch {current}")]
    ExpirationEpochTooSmall { expiry: Epoch, current: Epoch },

    #[error("invalid previous governance action id: {parent:?}")]
    InvalidPrevGovActionId { parent: Option<ProposalId> },

    #[error("invalid guardrails script hash: provided {provided:?}, expected {expected:?}")]
    InvalidGuardrailsScriptHash { provided: Option<Hash<SCRIPT>>, expected: Option<Hash<SCRIPT>> },

    #[error("era history error: {0}")]
    EraHistory(#[from] amaru_kernel::EraHistoryError),
}

pub(crate) fn execute<C>(
    context: &mut C,
    network: Network,
    protocol_parameters: &ProtocolParameters,
    era_history: &EraHistory,
    guardrail_script: Option<Hash<SCRIPT>>,
    transaction: (TransactionId, TransactionPointer),
    proposals: Option<Vec<Proposal>>,
) -> Result<(), InvalidProposals>
where
    C: ProposalsSlice + AccountsSlice + WitnessSlice + BalanceSlice,
{
    for (proposal_index, proposal) in proposals.unwrap_or_default().into_iter().enumerate() {
        validate_proposal(
            context,
            &proposal,
            network,
            protocol_parameters,
            era_history,
            guardrail_script,
            transaction.1,
        )?;

        if let Some(script_hash) = get_proposal_script_hash(&proposal) {
            context.require_script_witness(RequiredScript {
                hash: script_hash,
                index: proposal_index as u32,
                purpose: RedeemerTag::Propose,
                datum: MemoizedDatum::None,
            });
        }

        context.produce_lovelace(proposal.deposit);

        let pointer = ProposalPointer { transaction: transaction.1, proposal_index };
        let id = ProposalId { transaction_id: *transaction.0, proposal_index: proposal_index as u32 };
        context.acknowledge(id, pointer, proposal)
    }

    Ok(())
}

/// A simplified version of `execute` which only track the value produced by proposal deposits, in
/// order to verify transaction value preservation in the context of invalid transactions.
///
/// Note that we use the deposit value from the protocol parameters in that context, not the one in
/// the proposal since it isn't validated.
pub(crate) fn count_lovelace<C>(
    context: &mut C,
    protocol_parameters: &ProtocolParameters,
    proposals: Option<Vec<Proposal>>,
) where
    C: BalanceSlice,
{
    context.produce_lovelace(protocol_parameters.gov_action_deposit * proposals.unwrap_or_default().len() as u64);
}

fn validate_proposal<C>(
    context: &C,
    proposal: &Proposal,
    network: Network,
    protocol_parameters: &ProtocolParameters,
    era_history: &EraHistory,
    guardrail_script: Option<Hash<SCRIPT>>,
    pointer: TransactionPointer,
) -> Result<(), InvalidProposals>
where
    C: AccountsSlice + ProposalsSlice,
{
    if proposal.deposit != protocol_parameters.gov_action_deposit {
        return Err(InvalidProposals::IncorrectDeposit {
            provided: proposal.deposit,
            expected: protocol_parameters.gov_action_deposit,
        });
    }

    let kind = ProposalSlim::from(&proposal.gov_action);
    if !matches!(kind, ProposalSlim::Orphan) {
        let parent = proposal.parent();
        let follows_root = parent == context.roots().root_of(kind);
        let follows_in_flight = parent
            .and_then(|id| ProposalsSlice::lookup(context, id))
            .is_some_and(|in_flight| in_flight.same_lineage(kind));
        if !follows_root && !follows_in_flight {
            return Err(InvalidProposals::InvalidPrevGovActionId { parent: parent.cloned() });
        }
    }

    match Address::from_bytes(&proposal.reward_account[..]) {
        Some(Address::Stake(addr)) => {
            let actual = addr.network();

            let credential = StakeCredential::from(*(addr.payload()));

            if AccountsSlice::lookup(context, &credential).is_none() {
                return Err(InvalidProposals::ProposalReturnAccountDoesNotExist(credential));
            }

            if actual != network {
                return Err(InvalidProposals::ReturnAddressWrongNetwork { expected: network, actual });
            }
        }
        _ => return Err(InvalidProposals::MalformedReturnAddress),
    }

    match &proposal.gov_action {
        GovernanceAction::TreasuryWithdrawals(wdrls, policy) => {
            let mut any_positive = false;
            let mut missing = BTreeSet::new();

            for (account, coin) in wdrls.iter() {
                match Address::from_bytes(&account[..]) {
                    Some(Address::Stake(addr)) => {
                        let actual = addr.network();
                        if actual != network {
                            return Err(InvalidProposals::TreasuryWithdrawalWrongNetwork { expected: network, actual });
                        }

                        any_positive |= *coin > 0;

                        let credential = StakeCredential::from(*(addr.payload()));

                        if AccountsSlice::lookup(context, &credential).is_none() {
                            missing.insert(credential);
                        }
                    }
                    _ => return Err(InvalidProposals::MalformedReturnAddress),
                }
            }

            check_guardrails_script_hash(guardrail_script, *policy)?;

            if !any_positive {
                return Err(InvalidProposals::TreasuryWithdrawalsAllZeros);
            }

            if !missing.is_empty() {
                return Err(InvalidProposals::TreasuryWithdrawalReturnAccountsDoNotExist(missing));
            }
        }

        GovernanceAction::UpdateCommittee(_, removed, added, _) => {
            let added_keys: std::collections::BTreeSet<_> = added.iter().map(|(k, _)| k).collect();
            let removed_keys: std::collections::BTreeSet<_> = removed.iter().collect();
            if !added_keys.is_disjoint(&removed_keys) {
                return Err(InvalidProposals::ConflictingCommitteeUpdate);
            }

            // NOTE: conformance tests are brittle on this check due to era_history mismatch.
            // (see certificates.rs PoolRetirement comment for details)
            let current = era_history.slot_to_epoch(pointer.slot, pointer.slot)?;
            for (_, expiry) in added.iter() {
                if expiry <= &current {
                    return Err(InvalidProposals::ExpirationEpochTooSmall { expiry: *expiry, current });
                }
            }
        }

        GovernanceAction::NoConfidence(_) | GovernanceAction::NewConstitution(..) => {}

        GovernanceAction::HardForkInitiation(parent, new_version) => {
            let current_version = protocol_parameters.protocol_version;

            let last_proposed_version = pending_hard_fork_version(context, parent.as_ref()).unwrap_or(current_version);

            if new_version.major() > current_version.major() + 1 || !new_version.can_follow(last_proposed_version) {
                return Err(InvalidProposals::HardforkCantFollow {
                    last_proposed_version,
                    new_version: *new_version,
                    current_version,
                });
            }
        }

        GovernanceAction::ParameterChange(_, ppu, policy) => {
            ppu_well_formed(ppu)?;
            check_guardrails_script_hash(guardrail_script, *policy)?;
        }

        GovernanceAction::Information => {}
    }

    Ok(())
}

/// The protocol version a hard fork proposal must follow. A proposal chaining onto another hard
/// fork still in flight follows *that* proposal's version rather than the one currently in effect.
fn pending_hard_fork_version<C>(context: &C, parent: Option<&ProposalId>) -> Option<ProtocolVersion>
where
    C: ProposalsSlice,
{
    if parent == context.roots().hard_fork.as_ref() {
        return None;
    }

    match parent.and_then(|id| context.lookup(id)) {
        Some(ProposalSlim::HardFork(previous_version)) => Some(previous_version),
        // The lineage check in `validate_proposal` runs first, and rejects any parent that is
        // neither the hard fork root nor a hard fork proposal still in flight.
        parent => unreachable!("hardfork proposal follows neither its root nor an in-flight hardfork: {parent:?}"),
    }
}

/// Only `TreasuryWithdrawals` and `ParameterChange` proposals require the guardrails script hash.
/// They must match exactly, including in the case that the protocol hasn't enacted a constitution.
fn check_guardrails_script_hash(
    expected: Option<Hash<SCRIPT>>,
    provided: Option<Hash<SCRIPT>>,
) -> Result<(), InvalidProposals> {
    if provided != expected {
        return Err(InvalidProposals::InvalidGuardrailsScriptHash { provided, expected });
    }

    Ok(())
}

fn ppu_well_formed(ppu: &ProtocolParamUpdate) -> Result<(), InvalidProposals> {
    fn reject_zero(field: Option<u64>, field_name: &str) -> Result<(), InvalidProposals> {
        if field == Some(0) {
            return Err(InvalidProposals::MalformedProposal { reason: format!("{field_name} cannot be 0") });
        }
        Ok(())
    }

    reject_zero(ppu.max_block_body_size, "max_block_body_size")?;
    reject_zero(ppu.max_transaction_size, "max_transaction_size")?;
    reject_zero(ppu.max_block_header_size, "max_block_header_size")?;
    reject_zero(ppu.max_value_size, "max_value_size")?;
    reject_zero(ppu.collateral_percentage, "collateral_percentage")?;
    reject_zero(ppu.committee_term_limit, "committee_term_limit")?;
    reject_zero(ppu.governance_action_validity_period, "governance_action_validity_period")?;
    reject_zero(ppu.pool_deposit, "pool_deposit")?;
    reject_zero(ppu.governance_action_deposit, "governance_action_deposit")?;
    reject_zero(ppu.drep_deposit, "drep_deposit")?;

    reject_zero(ppu.ada_per_utxo_byte, "ada_per_utxo_byte")?;

    let is_empty = ppu.minfee_a.is_none()
        && ppu.minfee_b.is_none()
        && ppu.max_block_body_size.is_none()
        && ppu.max_transaction_size.is_none()
        && ppu.max_block_header_size.is_none()
        && ppu.key_deposit.is_none()
        && ppu.pool_deposit.is_none()
        && ppu.maximum_epoch.is_none()
        && ppu.desired_number_of_stake_pools.is_none()
        && ppu.pool_pledge_influence.is_none()
        && ppu.expansion_rate.is_none()
        && ppu.treasury_growth_rate.is_none()
        && ppu.min_pool_cost.is_none()
        && ppu.ada_per_utxo_byte.is_none()
        && ppu.cost_models_for_script_languages.is_none()
        && ppu.execution_costs.is_none()
        && ppu.max_tx_ex_units.is_none()
        && ppu.max_block_ex_units.is_none()
        && ppu.max_value_size.is_none()
        && ppu.collateral_percentage.is_none()
        && ppu.max_collateral_inputs.is_none()
        && ppu.pool_voting_thresholds.is_none()
        && ppu.drep_voting_thresholds.is_none()
        && ppu.min_committee_size.is_none()
        && ppu.committee_term_limit.is_none()
        && ppu.governance_action_validity_period.is_none()
        && ppu.governance_action_deposit.is_none()
        && ppu.drep_deposit.is_none()
        && ppu.drep_inactivity_period.is_none()
        && ppu.minfee_refscript_cost_per_byte.is_none();

    if is_empty {
        return Err(InvalidProposals::MalformedProposal { reason: "parameter update cannot be empty".into() });
    }

    Ok(())
}

fn get_proposal_script_hash(proposal: &Proposal) -> Option<Hash<SCRIPT>> {
    use amaru_kernel::GovernanceAction::*;

    match proposal.gov_action {
        ParameterChange(_, _, Some(gov_proposal_hash)) => Some(gov_proposal_hash),
        TreasuryWithdrawals(_, Some(gov_proposal_hash)) => Some(gov_proposal_hash),
        ParameterChange(..)
        | HardForkInitiation(..)
        | TreasuryWithdrawals(..)
        | NoConfidence(_)
        | UpdateCommittee(..)
        | NewConstitution(..)
        | Information => None,
    }
}
