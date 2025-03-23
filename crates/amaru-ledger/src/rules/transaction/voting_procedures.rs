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

use crate::context::WitnessSlice;
use amaru_kernel::{GovActionId, HasOwnership, NonEmptyKeyValuePairs, Voter, VotingProcedure};

pub(crate) fn execute(
    context: &mut impl WitnessSlice,
    voting_procedures: Option<&Vec<(Voter, NonEmptyKeyValuePairs<GovActionId, VotingProcedure>)>>,
) {
    if let Some(voting_procedures) = voting_procedures {
        voting_procedures.iter().for_each(|(voter, _)| {
            if let Some(credential) = voter.credential() {
                context.require_witness(credential);
            }
        });
    }
}
