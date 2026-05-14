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

use crate::{Block, ExUnits, HasRedeemers, WitnessSet, sum_ex_units};

pub trait HasExUnits {
    fn ex_units(&self) -> impl Iterator<Item = &ExUnits>;

    fn total_ex_units(&self) -> ExUnits {
        self.ex_units().fold(ExUnits { mem: 0, steps: 0 }, sum_ex_units)
    }
}

impl HasExUnits for Block {
    fn ex_units(&self) -> impl Iterator<Item = &ExUnits> {
        self.transaction_witnesses
            .iter()
            .filter_map(|witness_set| witness_set.as_ref().redeemer.as_ref())
            .flat_map(|redeemers| redeemers.iter_unique().map(|(_, ex_units, _)| ex_units))
    }
}

impl HasExUnits for WitnessSet {
    fn ex_units(&self) -> impl Iterator<Item = &ExUnits> {
        self.redeemer
            .as_ref()
            .into_iter()
            .flat_map(|redeemers| redeemers.iter_unique().map(|(_, ex_units, _)| ex_units))
    }
}
