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

use crate::context::{ProposalsSlice, WitnessSlice};
use amaru_kernel::{
    MemoizedDatum, Nullable, Proposal, ProposalId, ProposalPointer, RequiredScript, ScriptHash,
    ScriptPurpose, TransactionId, TransactionPointer,
};

pub(crate) fn execute<C>(
    context: &mut C,
    transaction: (TransactionId, TransactionPointer),
    proposals: Option<Vec<Proposal>>,
) where
    C: ProposalsSlice + WitnessSlice,
{
    for (proposal_index, proposal) in proposals.unwrap_or_default().into_iter().enumerate() {
        if let Some(script_hash) = get_proposal_script_hash(&proposal) {
            context.require_script_witness(RequiredScript {
                hash: script_hash,
                index: proposal_index as u32,
                purpose: ScriptPurpose::Propose,
                datum: MemoizedDatum::None,
            });
        }

        let pointer = ProposalPointer {
            transaction: transaction.1,
            proposal_index,
        };
        let id = ProposalId {
            transaction_id: transaction.0,
            action_index: proposal_index as u32,
        };
        context.acknowledge(id, pointer, proposal)
    }
}

fn get_proposal_script_hash(proposal: &Proposal) -> Option<ScriptHash> {
    match proposal.gov_action {
        amaru_kernel::GovAction::ParameterChange(_, _, Nullable::Some(gov_proposal_hash)) => {
            Some(gov_proposal_hash)
        }
        amaru_kernel::GovAction::TreasuryWithdrawals(_, Nullable::Some(gov_proposal_hash)) => {
            Some(gov_proposal_hash)
        }
        amaru_kernel::GovAction::ParameterChange(..)
        | amaru_kernel::GovAction::HardForkInitiation(..)
        | amaru_kernel::GovAction::TreasuryWithdrawals(..)
        | amaru_kernel::GovAction::NoConfidence(_)
        | amaru_kernel::GovAction::UpdateCommittee(..)
        | amaru_kernel::GovAction::NewConstitution(..)
        | amaru_kernel::GovAction::Information => None,
    }
}

#[cfg(test)]
mod tests {
    use std::mem;

    use crate::{context::assert::AssertValidationContext, rules::tests::fixture_context};
    use amaru_kernel::{
        KeepRaw, MintedTransactionBody, OriginalHash, Slot, TransactionPointer, include_cbor,
        include_json, json,
    };
    use amaru_tracing_json::assert_trace;
    use test_case::test_case;

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
                include_cbor!(concat!(
                    "transactions/preprod/",
                    $hash,
                    "/",
                    $variant,
                    "/tx.cbor"
                )),
                $pointer,
                include_json!(concat!(
                    "transactions/preprod/",
                    $hash,
                    "/",
                    $variant,
                    "/expected.traces"
                )),
            )
        };
    }

    #[test_case(fixture!("e974fecbf45ac386a76605e9e847a2e5d27c007fdd0be674cbad538e0c35fe01", TransactionPointer {
        slot: Slot::from(74013957),
        transaction_index: 0,
    }); "happy path")]

    fn test_proposals(
        (mut ctx, tx, tx_pointer, expected_traces): (
            AssertValidationContext,
            KeepRaw<'_, MintedTransactionBody<'_>>,
            TransactionPointer,
            Vec<json::Value>,
        ),
    ) {
        assert_trace(
            || {
                super::execute(
                    &mut ctx,
                    (todo!() /*tx.original_hash()*/, tx_pointer),
                    mem::take(&mut tx.unwrap().proposal_procedures).map(|xs| xs.to_vec()),
                )
            },
            expected_traces,
        )
    }
}
