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

pub use pallas_primitives::conway::DRep;

#[cfg(any(test, feature = "test-utils"))]
pub use tests::*;

#[cfg(any(test, feature = "test-utils"))]
mod tests {
    use crate::{DRep, any_hash28};
    use proptest::prelude::*;

    pub fn any_drep() -> impl Strategy<Value = DRep> {
        prop_oneof![
            any_hash28().prop_map(DRep::Key),
            any_hash28().prop_map(DRep::Script),
            Just(DRep::Abstain),
            Just(DRep::NoConfidence),
        ]
    }
}
