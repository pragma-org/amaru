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

pub use pallas_primitives::conway::CostModels;
#[cfg(any(test, feature = "test-utils"))]
pub use proxy::*;

#[cfg(any(test, feature = "test-utils"))]
mod proxy {
    use serde::Deserialize;

    use super::CostModels;
    use crate::{CostModel, utils::serde::HasProxy};

    /// Fixture JSON shape: Plutus version keys are camelCase (`plutusV1`, `plutusV2`, `plutusV3`).
    #[derive(Deserialize)]
    pub struct CostModelsProxy {
        #[serde(rename = "plutusV1")]
        plutus_v1: Option<CostModel>,
        #[serde(rename = "plutusV2")]
        plutus_v2: Option<CostModel>,
        #[serde(rename = "plutusV3")]
        plutus_v3: Option<CostModel>,
    }

    impl From<CostModelsProxy> for CostModels {
        fn from(p: CostModelsProxy) -> Self {
            CostModels { plutus_v1: p.plutus_v1, plutus_v2: p.plutus_v2, plutus_v3: p.plutus_v3 }
        }
    }

    impl HasProxy for CostModels {
        type Proxy = CostModelsProxy;
    }
}
