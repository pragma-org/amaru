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
    use crate::{Bytes, Hash, RewardAccount, StakePayload, new_stake_address, tests::any_network};
    use proptest::{prelude::*, prop_compose};

    prop_compose! {
        pub fn any_reward_account()(
            network in any_network(),
            credential in any::<[u8; 28]>(),
            is_script in any::<bool>(),
        ) -> RewardAccount {
            let payload = if is_script {
                StakePayload::Script
            } else {
                StakePayload::Stake
            }(Hash::new(credential));

            Bytes::from(new_stake_address(network, payload).to_vec())
        }
    }
}
