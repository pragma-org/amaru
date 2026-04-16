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
    Address, GovernanceAction, HasNetwork, Hash, Lovelace, MemoizedDatum, Network, Nullable, Proposal, ProposalId,
    ProposalPointer, ProtocolParameters, ProtocolVersion, RequiredScript, ScriptPurpose, TransactionId,
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

    #[error("conflicting committee update: members appear in both add and remove sets")]
    ConflictingCommitteeUpdate,

    #[error("disallowed proposal during bootstrap phase")]
    DisallowedDuringBootstrap,

    #[error("hardfork version {new:?} cannot follow current version {current:?}")]
    HardforkCantFollow { current: ProtocolVersion, new: ProtocolVersion },
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

    let reward_address =
        Address::from_bytes(&proposal.reward_account[..]).map_err(|_| InvalidProposals::MalformedReturnAddress)?;
    if reward_address.has_network() != network {
        return Err(InvalidProposals::ReturnAddressWrongNetwork {
            expected: network,
            actual: reward_address.has_network(),
        });
    }

    let is_bootstrap = protocol_parameters.protocol_version.0 == 9;

    match &proposal.gov_action {
        GovernanceAction::TreasuryWithdrawals(wdrls, _) => {
            if is_bootstrap {
                return Err(InvalidProposals::DisallowedDuringBootstrap);
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

        GovernanceAction::ParameterChange(..) | GovernanceAction::Information => {}
    }

    Ok(())
}

fn pv_can_follow(current: ProtocolVersion, new: ProtocolVersion) -> bool {
    let (cur_major, cur_minor) = current;
    let (new_major, new_minor) = new;
    (new_major == cur_major + 1 && new_minor == 0) || (new_major == cur_major && new_minor == cur_minor + 1)
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
