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

pub use pallas_primitives::conway::ProposalProcedure as Proposal;

#[cfg(any(test, feature = "test-utils"))]
pub use tests::*;

#[cfg(any(test, feature = "test-utils"))]
mod tests {
    use crate::{Lovelace, Proposal, any_anchor, any_gov_action, any_reward_account};
    use proptest::{prelude::*, prop_compose};

    prop_compose! {
        pub fn any_proposal()(
            deposit in any::<Lovelace>(),
            reward_account in any_reward_account(),
            gov_action in any_gov_action(),
            anchor in any_anchor(),
        ) -> Proposal {
            Proposal {
                deposit,
                reward_account,
                gov_action,
                anchor,
            }
        }
    }
}
