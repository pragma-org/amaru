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

pub use pallas_primitives::conway::ExUnits;

/// Create a new `ExUnits` that is the sum of two `ExUnits`
pub fn sum_ex_units(left: ExUnits, right: &ExUnits) -> ExUnits {
    ExUnits { mem: left.mem + right.mem, steps: left.steps + right.steps }
}

#[cfg(any(test, feature = "test-utils"))]
pub use proxy::*;

#[cfg(any(test, feature = "test-utils"))]
mod proxy {
    use serde::Deserialize;

    use super::ExUnits;
    use crate::utils::serde::HasProxy;

    /// Fixture JSON shape `{ "memory": <u64>, "cpu": <u64> }`.
    #[derive(Deserialize)]
    pub struct ExUnitsProxy {
        memory: u64,
        cpu: u64,
    }

    impl From<ExUnitsProxy> for ExUnits {
        fn from(p: ExUnitsProxy) -> Self {
            ExUnits { mem: p.memory, steps: p.cpu }
        }
    }

    impl HasProxy for ExUnits {
        type Proxy = ExUnitsProxy;
    }
}
