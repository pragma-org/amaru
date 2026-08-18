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

use std::{fmt, ops::Add};

use crate::cbor;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, cbor::Encode, cbor::Decode)]
#[cbor(context_bound = "crate::cbor::HasProtocolVersion")]
pub struct ExUnits {
    #[n(0)]
    pub mem: u64,
    #[n(1)]
    pub steps: u64,
}

impl Add for &ExUnits {
    type Output = ExUnits;

    fn add(self, rhs: Self) -> Self::Output {
        ExUnits { mem: self.mem + rhs.mem, steps: self.steps + rhs.steps }
    }
}

impl fmt::Display for ExUnits {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{{mem={}, cpu={}}}", self.mem, self.steps)
    }
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
