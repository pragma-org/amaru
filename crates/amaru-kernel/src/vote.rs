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

#[cfg(any(test, feature = "test-utils"))]
pub mod tests {
    use crate::Vote;
    use proptest::prelude::*;

    pub static VOTE_YES: Vote = Vote::Yes;
    pub static VOTE_NO: Vote = Vote::No;
    pub static VOTE_ABSTAIN: Vote = Vote::Abstain;

    pub fn any_vote() -> impl Strategy<Value = Vote> {
        prop_oneof![Just(Vote::Yes), Just(Vote::No), Just(Vote::Abstain)]
    }

    pub fn any_vote_ref() -> impl Strategy<Value = &'static Vote> {
        prop_oneof![Just(&VOTE_YES), Just(&VOTE_NO), Just(&VOTE_ABSTAIN)]
    }
}
