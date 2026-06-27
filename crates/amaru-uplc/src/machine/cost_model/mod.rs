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

use amaru_kernel::{PlutusVersion, ProtocolVersion, protocol_version};

use crate::machine::{
    Semantics,
    cost_model::{builtin_costs::BuiltinCosts, machine_costs::MachineCosts},
};

pub mod builtin_costs;
pub mod machine_costs;

pub mod cost_map;
pub mod costing;
pub mod ex_budget;

mod param_name;
pub use param_name::*;

mod step_kind;
pub use step_kind::*;

pub mod value;

#[derive(Debug, PartialEq, Default)]
pub struct CostModel {
    pub semantics: Semantics,
    pub machine_costs: MachineCosts,
    pub builtin_costs: BuiltinCosts,
}

impl CostModel {
    /// Create a new `CostModel` for a given Plutus version and Protocol version. These two
    /// versions determines how the array of cost numbers should be interpreted (how positions map
    /// to specific parameters).
    pub fn new(
        plutus_version: PlutusVersion,
        protocol_version: ProtocolVersion,
        costs: &[i64],
    ) -> Result<Self, ParamName> {
        let semantics = Semantics::new(plutus_version, protocol_version);
        let cost_map = ParamName::new_cost_map(plutus_version, costs);
        Ok(Self {
            semantics,
            machine_costs: MachineCosts::new(&cost_map, plutus_version)?,
            builtin_costs: BuiltinCosts::new(&cost_map, semantics, protocol_version)?,
        })
    }

    /// Latest cost models for Plutus V1
    pub const DEFAULT_V1: [i64; 332] = amaru_kernel::protocol_parameters::DEFAULT_V1_COST_MODEL;

    pub fn v1() -> Self {
        Self::predefined(PlutusVersion::V1, protocol_version::DEFAULT, &Self::DEFAULT_V1[..])
    }

    /// Latest cost models for Plutus V2
    pub const DEFAULT_V2: [i64; 332] = amaru_kernel::protocol_parameters::DEFAULT_V2_COST_MODEL;

    pub fn v2() -> Self {
        Self::predefined(PlutusVersion::V2, protocol_version::DEFAULT, &Self::DEFAULT_V2[..])
    }

    /// Latest cost models for Plutus V3
    pub const DEFAULT_V3: [i64; 350] = amaru_kernel::protocol_parameters::DEFAULT_V3_COST_MODEL;

    pub fn v3() -> Self {
        Self::predefined(PlutusVersion::V3, protocol_version::DEFAULT, &Self::DEFAULT_V3[..])
    }

    fn predefined(plutus_version: PlutusVersion, protocol_version: ProtocolVersion, costs: &[i64]) -> Self {
        Self::new(plutus_version, protocol_version, costs).unwrap_or_else(|param| {
            unreachable!(
                "invalid defaults for Plutus version = {:?}, protocol version = {:?}; missing parameter = {:?}",
                plutus_version, protocol_version, param
            )
        })
    }
}
