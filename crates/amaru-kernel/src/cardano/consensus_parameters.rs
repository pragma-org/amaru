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

use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{EraHistory, GlobalParameters, KesPeriod, Slot, maths::FixedDecimal};

/// This data type encapsulates the parameters needed by the consensus layer to operate.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConsensusParameters {
    randomness_stabilization_window: u64,
    slots_per_kes_period: u64,
    max_kes_evolution: u64,
    active_slot_coeff: SerializedFixedDecimal,
    active_slot_coeff_ln_complement: SerializedFixedDecimal,
    era_history: EraHistory,
}

impl ConsensusParameters {
    /// Create new consensus parameters from the given global parameters.
    pub fn new(global_parameters: GlobalParameters, era_history: &EraHistory) -> Self {
        Self::create(
            global_parameters.randomness_stabilization_window(),
            global_parameters.slots_per_kes_period,
            global_parameters.max_kes_evolution as u64,
            global_parameters.active_slot_coeff_inverse,
            era_history,
        )
    }

    /// Create new consensus parameters from individual values.
    pub fn create(
        randomness_stabilization_window: u64,
        slots_per_kes_period: u64,
        max_kes_evolution: u64,
        active_slot_coeff_inverse: u64,
        era_history: &EraHistory,
    ) -> ConsensusParameters {
        let active_slot_coeff = FixedDecimal::one() / &FixedDecimal::from(active_slot_coeff_inverse);

        assert!(active_slot_coeff != FixedDecimal::ZERO, "active slot coefficient cannot be null");

        let active_slot_coeff_ln_complement = if &active_slot_coeff >= FixedDecimal::one() {
            FixedDecimal::ZERO
        } else {
            (FixedDecimal::one() - &active_slot_coeff).ln()
        };

        Self {
            randomness_stabilization_window,
            slots_per_kes_period,
            max_kes_evolution,
            active_slot_coeff: SerializedFixedDecimal(active_slot_coeff),
            active_slot_coeff_ln_complement: SerializedFixedDecimal(active_slot_coeff_ln_complement),
            era_history: era_history.clone(),
        }
    }

    pub fn era_history(&self) -> &EraHistory {
        &self.era_history
    }

    pub fn randomness_stabilization_window(&self) -> u64 {
        self.randomness_stabilization_window
    }

    pub fn slot_to_kes_period(&self, slot: Slot) -> KesPeriod {
        KesPeriod::from(u64::from(slot) / self.slots_per_kes_period)
    }

    pub fn max_kes_evolutions(&self) -> u64 {
        self.max_kes_evolution
    }

    pub fn active_slot_coeff(&self) -> &FixedDecimal {
        &self.active_slot_coeff.0
    }

    pub fn active_slot_coeff_ln_complement(&self) -> &FixedDecimal {
        &self.active_slot_coeff_ln_complement.0
    }
}

#[derive(Clone, Debug, PartialEq)]
struct SerializedFixedDecimal(FixedDecimal);

impl Serialize for SerializedFixedDecimal {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'a> Deserialize<'a> for SerializedFixedDecimal {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'a>,
    {
        let s = String::deserialize(deserializer)?;
        FixedDecimal::from_str(&s).map(SerializedFixedDecimal).map_err(serde::de::Error::custom)
    }
}
