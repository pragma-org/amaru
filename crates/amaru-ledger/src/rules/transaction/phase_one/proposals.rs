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

use amaru_kernel::{
    Address, GovernanceAction, Hash, Lovelace, MemoizedDatum, Network, Nullable, Proposal, ProposalId, ProposalPointer,
    ProtocolParamUpdate, ProtocolParameters, ProtocolVersion, RequiredScript, ScriptPurpose, TransactionId,
    TransactionPointer, size::SCRIPT,
};
use thiserror::Error;

use crate::context::{ProposalsSlice, WitnessSlice};

#[derive(Debug, Error)]
pub enum InvalidProposals {
    #[error("incorrect proposal deposit: provided {provided}, expected {expected}")]
    IncorrectDeposit { provided: Lovelace, expected: Lovelace },

    #[error("proposal return address has wrong network: expected {expected:?}, actual {actual:?}")]
    ReturnAddressWrongNetwork { expected: Network, actual: Network },

    #[error("proposal return address is malformed")]
    MalformedReturnAddress,

    #[error("treasury withdrawals total is zero")]
    ZeroTreasuryWithdrawals,

    #[error("treasury withdrawal address has wrong network: expected {expected:?}, actual {actual:?}")]
    TreasuryWithdrawalWrongNetwork { expected: Network, actual: Network },

    #[error("conflicting committee update: members appear in both add and remove sets")]
    ConflictingCommitteeUpdate,

    #[error("disallowed proposal during bootstrap phase")]
    DisallowedDuringBootstrap,

    #[error("hardfork version {new:?} cannot follow current version {current:?}")]
    HardforkCantFollow { current: ProtocolVersion, new: ProtocolVersion },

    #[error("malformed parameter change proposal: {reason}")]
    MalformedProposal { reason: String },
}

pub(crate) fn execute<C>(
    context: &mut C,
    network: Network,
    protocol_parameters: &ProtocolParameters,
    transaction: (TransactionId, TransactionPointer),
    proposals: Option<Vec<Proposal>>,
) -> Result<(), InvalidProposals>
where
    C: ProposalsSlice + WitnessSlice,
{
    for (proposal_index, proposal) in proposals.unwrap_or_default().into_iter().enumerate() {
        validate_proposal(&proposal, network, protocol_parameters)?;

        if let Some(script_hash) = get_proposal_script_hash(&proposal) {
            context.require_script_witness(RequiredScript {
                hash: script_hash,
                index: proposal_index as u32,
                purpose: ScriptPurpose::Propose,
                datum: MemoizedDatum::None,
            });
        }

        let pointer = ProposalPointer { transaction: transaction.1, proposal_index };
        let id = ProposalId { transaction_id: transaction.0, action_index: proposal_index as u32 };
        context.acknowledge(id, pointer, proposal)
    }

    Ok(())
}

fn validate_proposal(
    proposal: &Proposal,
    network: Network,
    protocol_parameters: &ProtocolParameters,
) -> Result<(), InvalidProposals> {
    if proposal.deposit != protocol_parameters.gov_action_deposit {
        return Err(InvalidProposals::IncorrectDeposit {
            provided: proposal.deposit,
            expected: protocol_parameters.gov_action_deposit,
        });
    }

    match Address::from_bytes(&proposal.reward_account[..]) {
        Ok(Address::Stake(addr)) => {
            let actual = addr.network();
            if actual != network {
                return Err(InvalidProposals::ReturnAddressWrongNetwork { expected: network, actual });
            }
        }
        _ => return Err(InvalidProposals::MalformedReturnAddress),
    }

    let is_bootstrap = protocol_parameters.protocol_version.0 == 9;

    match &proposal.gov_action {
        GovernanceAction::TreasuryWithdrawals(wdrls, _) => {
            if is_bootstrap {
                return Err(InvalidProposals::DisallowedDuringBootstrap);
            }
            for (account, _) in wdrls.iter() {
                match Address::from_bytes(&account[..]) {
                    Ok(Address::Stake(addr)) => {
                        let actual = addr.network();
                        if actual != network {
                            return Err(InvalidProposals::TreasuryWithdrawalWrongNetwork { expected: network, actual });
                        }
                    }
                    _ => return Err(InvalidProposals::MalformedReturnAddress),
                }
            }
            if !wdrls.iter().any(|(_, coin)| *coin > 0) {
                return Err(InvalidProposals::ZeroTreasuryWithdrawals);
            }
        }

        GovernanceAction::UpdateCommittee(_, removed, added, _) => {
            if is_bootstrap {
                return Err(InvalidProposals::DisallowedDuringBootstrap);
            }
            let added_keys: std::collections::BTreeSet<_> = added.iter().map(|(k, _)| k).collect();
            let removed_keys: std::collections::BTreeSet<_> = removed.iter().collect();
            if !added_keys.is_disjoint(&removed_keys) {
                return Err(InvalidProposals::ConflictingCommitteeUpdate);
            }
        }

        GovernanceAction::NoConfidence(_) | GovernanceAction::NewConstitution(..) => {
            if is_bootstrap {
                return Err(InvalidProposals::DisallowedDuringBootstrap);
            }
        }

        GovernanceAction::HardForkInitiation(_, new_version) => {
            if !pv_can_follow(protocol_parameters.protocol_version, *new_version) {
                return Err(InvalidProposals::HardforkCantFollow {
                    current: protocol_parameters.protocol_version,
                    new: *new_version,
                });
            }
        }

        GovernanceAction::ParameterChange(_, ppu, _) => {
            ppu_well_formed(protocol_parameters.protocol_version, ppu)?;
        }

        GovernanceAction::Information => {}
    }

    Ok(())
}

fn pv_can_follow(current: ProtocolVersion, new: ProtocolVersion) -> bool {
    let (cur_major, cur_minor) = current;
    let (new_major, new_minor) = new;
    (new_major == cur_major + 1 && new_minor == 0) || (new_major == cur_major && new_minor == cur_minor + 1)
}

fn ppu_well_formed(pv: ProtocolVersion, ppu: &ProtocolParamUpdate) -> Result<(), InvalidProposals> {
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

    if pv.0 != 9 {
        reject_zero(ppu.ada_per_utxo_byte, "ada_per_utxo_byte")?;
    }

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
        ParameterChange(_, _, Nullable::Some(gov_proposal_hash)) => Some(gov_proposal_hash),
        TreasuryWithdrawals(_, Nullable::Some(gov_proposal_hash)) => Some(gov_proposal_hash),
        ParameterChange(..)
        | HardForkInitiation(..)
        | TreasuryWithdrawals(..)
        | NoConfidence(_)
        | UpdateCommittee(..)
        | NewConstitution(..)
        | Information => None,
    }
}

#[cfg(test)]
mod tests {
    use std::mem;

    use amaru_kernel::{Slot, TransactionBody, TransactionPointer, include_cbor, include_json, json};
    use amaru_tracing_json::assert_trace;
    use test_case::test_case;

    use crate::{context::assert::AssertValidationContext, rules::tests::fixture_context};

    macro_rules! fixture {
        ($hash:literal, $pointer:expr) => {
            (
                fixture_context!($hash),
                include_cbor!(concat!("transactions/preprod/", $hash, "/tx.cbor")),
                $pointer,
                include_json!(concat!("transactions/preprod/", $hash, "/expected.traces")),
            )
        };
        ($hash:literal, $variant:literal, $pointer:expr) => {
            (
                fixture_context!($hash, $variant),
                include_cbor!(concat!("transactions/preprod/", $hash, "/", $variant, "/tx.cbor")),
                $pointer,
                include_json!(concat!("transactions/preprod/", $hash, "/", $variant, "/expected.traces")),
            )
        };
    }

    #[test_case(fixture!("e974fecbf45ac386a76605e9e847a2e5d27c007fdd0be674cbad538e0c35fe01", TransactionPointer {
        slot: Slot::from(74013957),
        transaction_index: 0,
    }); "happy path")]

    fn test_proposals(
        (mut ctx, mut tx, tx_pointer, expected_traces): (
            AssertValidationContext,
            TransactionBody,
            TransactionPointer,
            Vec<json::Value>,
        ),
    ) {
        assert_trace(
            || {
                super::execute(
                    &mut ctx,
                    amaru_kernel::Network::Testnet,
                    &amaru_kernel::PREPROD_DEFAULT_PROTOCOL_PARAMETERS,
                    (tx.id(), tx_pointer),
                    mem::take(&mut tx.proposals).map(|xs| xs.to_vec()),
                )
                .expect("validation should not fail for this fixture")
            },
            expected_traces,
        )
    }
}
