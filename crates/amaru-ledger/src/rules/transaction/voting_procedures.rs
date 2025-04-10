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
use amaru_kernel::{NonEmptyKeyValuePairs, ProposalId, StakeCredential, Voter, VotingProcedure};

pub(crate) fn execute<C>(
    context: &mut C,
    voting_procedures: Option<&Vec<(Voter, NonEmptyKeyValuePairs<ProposalId, VotingProcedure>)>>,
) where
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
