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

use std::fmt;

use crate::{RationalNumber, cbor};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, cbor::Encode, cbor::Decode)]
#[cbor(context_bound = "crate::cbor::HasProtocolVersion")]
pub struct ExUnitPrices {
    #[n(0)]
    pub mem_price: RationalNumber,

    #[n(1)]
    pub step_price: RationalNumber,
}

impl fmt::Display for ExUnitPrices {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{{mem={}, cpu={}}}", self.mem_price, self.step_price)
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub use proxy::*;

#[cfg(any(test, feature = "test-utils"))]
mod proxy {
    use serde::Deserialize;

    use super::ExUnitPrices;
    use crate::{RationalNumber, utils::serde::HasProxy};

    /// Fixture JSON shape `{ "memory": <ratio>, "cpu": <ratio> }`.
    #[derive(Deserialize)]
    pub struct ExUnitPricesProxy {
        memory: RationalNumber,
        cpu: RationalNumber,
    }

    impl From<ExUnitPricesProxy> for ExUnitPrices {
        fn from(p: ExUnitPricesProxy) -> Self {
            ExUnitPrices { mem_price: p.memory, step_price: p.cpu }
        }
    }

    impl HasProxy for ExUnitPrices {
        type Proxy = ExUnitPricesProxy;
    }
}
