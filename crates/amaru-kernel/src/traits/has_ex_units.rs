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

use crate::{Block, ExUnits, HasRedeemers};

pub trait HasExUnits {
    fn ex_units(&self) -> Vec<&ExUnits>;
}

impl HasExUnits for Block {
    fn ex_units(&self) -> Vec<&ExUnits> {
        self.transaction_witnesses
            .iter()
            .fold(Vec::new(), |mut acc: Vec<&ExUnits>, witness_set| {
                if let Some(witnesses) = &witness_set.redeemer {
                    acc.extend(witnesses.redeemers().values().map(|(ex_units, _)| ex_units));
                }
                acc
            })
    }
}
