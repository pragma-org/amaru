// Copyright 2026 PRAGMA
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

use crate::{Hash, size::VRF_KEY};

/// A slim representation of a pool's state, mostly useful for rules validations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PoolSlim {
    pub vrf: Hash<VRF_KEY>,
    pub has_pending_updates: bool,
}

#[cfg(any(test, feature = "test-utils"))]
pub use tests::*;

#[cfg(any(test, feature = "test-utils"))]
mod tests {
    use proptest::prelude::*;

    use crate::{PoolSlim, any_hash32};

    prop_compose! {
        pub fn any_pool_slim()(
            vrf in any_hash32(),
            has_pending_updates in any::<bool>(),
        ) -> PoolSlim {
            PoolSlim { vrf, has_pending_updates }
        }
    }
}
