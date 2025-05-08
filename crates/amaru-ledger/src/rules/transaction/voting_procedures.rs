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

use crate::context::{DRepsSlice, WitnessSlice};
use amaru_kernel::{StakeCredential, Voter, VotingProcedures};

pub(crate) fn execute<C>(context: &mut C, voting_procedures: Option<&VotingProcedures>)
where
    C: WitnessSlice + DRepsSlice,
{
    if let Some(voting_procedures) = voting_procedures {
        voting_procedures.iter().for_each(|(voter, _)| {
            let credential = match voter {
                Voter::ConstitutionalCommitteeKey(hash) | Voter::StakePoolKey(hash) => {
                    StakeCredential::AddrKeyhash(*hash)
                }
                Voter::ConstitutionalCommitteeScript(hash) => StakeCredential::ScriptHash(*hash),
                Voter::DRepKey(hash) => {
                    let credential = StakeCredential::AddrKeyhash(*hash);
                    context.vote(credential.clone());
                    credential
                }

                Voter::DRepScript(hash) => {
                    let credential = StakeCredential::ScriptHash(*hash);
                    context.vote(credential.clone());
                    credential
                }
            };

            context.require_witness(credential);
        });
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crate::context::assert::{AssertPreparationContext, AssertValidationContext};
    use amaru_kernel::{include_cbor, include_json, json, KeepRaw, TransactionBody};
    use test_case::test_case;
    use tracing_json::assert_trace;

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
        (tx, expected_traces): (KeepRaw<'_, TransactionBody<'_>>, Vec<json::Value>),
    ) {
        assert_trace(
            || {
                let mut validation_context =
                    AssertValidationContext::from(AssertPreparationContext {
                        utxo: BTreeMap::new(),
                    });
                super::execute(&mut validation_context, tx.voting_procedures.as_ref())
            },
            expected_traces,
        );
    }
}
