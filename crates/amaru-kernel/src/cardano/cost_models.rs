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

use crate::{CostModel, cbor};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, cbor::Encode, cbor::Decode)]
#[cbor(context_bound = "crate::cbor::HasProtocolVersion")]
#[cbor(map)]
pub struct CostModels {
    #[n(0)]
    pub plutus_v1: Option<CostModel>,

    #[n(1)]
    pub plutus_v2: Option<CostModel>,

    #[n(2)]
    pub plutus_v3: Option<CostModel>,
}

impl fmt::Display for CostModels {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // NOTE: destructuring for completeness static checks
        let CostModels { plutus_v1, plutus_v2, plutus_v3 } = self;

        let mut needs_separator = false;

        if let Some(cost_model) = plutus_v1 {
            write!(f, "plutus_v1 = {:?}", cost_model)?;
            needs_separator = true;
        }

        if let Some(cost_model) = plutus_v2 {
            write!(f, "{}plutus_v2 = {:?}", if needs_separator { ", " } else { "" }, cost_model)?;
            needs_separator = true;
        }

        if let Some(cost_model) = plutus_v3 {
            write!(f, "{}plutus_v3 = {:?}", if needs_separator { ", " } else { "" }, cost_model)?;
        }

        Ok(())
    }
}
