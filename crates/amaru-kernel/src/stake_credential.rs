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
    use crate::{Hash, StakeCredential};
    use proptest::prelude::*;

    pub fn any_stake_credential() -> impl Strategy<Value = StakeCredential> {
        prop_oneof![
            any::<[u8; 28]>().prop_map(|hash| StakeCredential::AddrKeyhash(Hash::new(hash))),
            any::<[u8; 28]>().prop_map(|hash| StakeCredential::ScriptHash(Hash::new(hash))),
        ]
    }
}
