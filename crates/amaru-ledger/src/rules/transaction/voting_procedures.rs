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
    HasStakeCredential, MemoizedDatum, NonEmptyKeyValuePairs, ProposalId, RequiredScript,
    ScriptPurpose, StakeCredential, Voter, VotingProcedure,
};

pub(crate) fn execute<C>(
    context: &mut C,
    voting_procedures: Option<Vec<(Voter, NonEmptyKeyValuePairs<ProposalId, VotingProcedure>)>>,
) where
    C: WitnessSlice + ProposalsSlice,
{
    if let Some(voting_procedures) = voting_procedures {
        voting_procedures
            .into_iter()
            .enumerate()
            .for_each(|(index, (voter, votes))| {
                match voter.stake_credential() {
                    StakeCredential::ScriptHash(hash) => {
                        context.require_script_witness(RequiredScript {
                            hash,
                            index: index as u32,
                            purpose: ScriptPurpose::Vote,
                            datum: MemoizedDatum::None,
                        });
                    }
                    StakeCredential::AddrKeyhash(hash) => context.require_vkey_witness(hash),
                }

                votes.into_iter().for_each(|(proposal_id, ballot)| {
                    context.vote(
                        proposal_id,
                        voter.clone(),
                        ballot.vote,
                        Option::from(ballot.anchor),
                    );
                })
            });
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crate::context::assert::{AssertPreparationContext, AssertValidationContext};
    use amaru_kernel::{KeepRaw, MintedTransactionBody, include_cbor, include_json, json};
    use amaru_tracing_json::assert_trace;
    use test_case::test_case;

    macro_rules! fixture {
        ($hash:literal, $variant:literal) => {
            (
                include_cbor!(concat!(
                    "transactions/preprod/",
                    $hash,
                    "/",
                    $variant,
                    "/tx.cbor"
                )),
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

    #[test_case(fixture!("278d887adc913416e6851106e7ce6e89f29aa7531b93d11e1986550e7a128a2f", "cc-key"); "CC Key")]
    #[test_case(fixture!("278d887adc913416e6851106e7ce6e89f29aa7531b93d11e1986550e7a128a2f", "cc-script"); "CC Script")]
    #[test_case(fixture!("278d887adc913416e6851106e7ce6e89f29aa7531b93d11e1986550e7a128a2f", "drep-key"); "DRep Key")]
    #[test_case(fixture!("278d887adc913416e6851106e7ce6e89f29aa7531b93d11e1986550e7a128a2f", "drep-script"); "DRep Script")]
    #[test_case(fixture!("278d887adc913416e6851106e7ce6e89f29aa7531b93d11e1986550e7a128a2f", "spo-key"); "SPO Key")]
    fn voting_procedures(
        (tx, expected_traces): (KeepRaw<'_, MintedTransactionBody<'_>>, Vec<json::Value>),
    ) {
        assert_trace(
            || {
                let mut validation_context =
                    AssertValidationContext::from(AssertPreparationContext {
                        utxo: BTreeMap::new(),
                    });
                super::execute(
                    &mut validation_context,
                    tx.voting_procedures.as_deref().cloned(),
                )
            },
            expected_traces,
        );
    }
}
