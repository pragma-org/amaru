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

use amaru_kernel::{
    PlutusVersion, ProtocolVersion,
    cost_model::{
        DEFAULT_COST_MODEL_A_V1, DEFAULT_COST_MODEL_A_V2, DEFAULT_COST_MODEL_B_V1, DEFAULT_COST_MODEL_B_V2,
        DEFAULT_COST_MODEL_C_V3, DEFAULT_COST_MODEL_D_V1, DEFAULT_COST_MODEL_D_V2, DEFAULT_COST_MODEL_E_V3,
    },
    protocol_version,
};

use crate::machine::{
    CaseOnConstants, Semantics,
    cost_model::{builtin_costs::BuiltinCosts, machine_costs::MachineCosts},
};

pub mod builtin_costs;
pub mod machine_costs;

pub mod costing;
pub mod ex_budget;

pub mod cost_argument;

mod param_name;
pub use param_name::*;

mod step_kind;
pub use step_kind::*;

#[derive(Debug, PartialEq, Default, Clone)]
pub struct CostModel {
    pub semantics: Semantics,
    pub case_on_constants: CaseOnConstants,
    pub machine_costs: MachineCosts,
    pub builtin_costs: BuiltinCosts,
}

impl CostModel {
    /// Create a new `CostModel` for a given Plutus version and Protocol version. These two
    /// versions determines how the array of cost numbers should be interpreted (how positions map
    /// to specific parameters).
    pub fn new(plutus_version: PlutusVersion, protocol_version: ProtocolVersion, costs: &[i64]) -> Self {
        let semantics = Semantics::new(plutus_version, protocol_version);
        let cost_map = ParamName::new_cost_map(plutus_version, costs);
        Self {
            semantics,
            case_on_constants: CaseOnConstants::new(protocol_version),
            machine_costs: MachineCosts::new(&cost_map),
            builtin_costs: BuiltinCosts::new(&cost_map, semantics),
        }
    }

    /// Build a cost model for the semantics selected by a Plutus and protocol version.
    pub fn default_for(plutus_version: PlutusVersion, protocol_version: ProtocolVersion) -> Self {
        let costs: &[i64] = match (Semantics::new(plutus_version, protocol_version), plutus_version) {
            (Semantics::A, PlutusVersion::V1) => &DEFAULT_COST_MODEL_A_V1,
            (Semantics::A, PlutusVersion::V2) => &DEFAULT_COST_MODEL_A_V2,
            (Semantics::B, PlutusVersion::V1) => &DEFAULT_COST_MODEL_B_V1,
            (Semantics::B, PlutusVersion::V2) => &DEFAULT_COST_MODEL_B_V2,
            (Semantics::C, PlutusVersion::V3) => &DEFAULT_COST_MODEL_C_V3,
            (Semantics::D, PlutusVersion::V1) => &DEFAULT_COST_MODEL_D_V1,
            (Semantics::D, PlutusVersion::V2) => &DEFAULT_COST_MODEL_D_V2,
            (Semantics::E, PlutusVersion::V3) => &DEFAULT_COST_MODEL_E_V3,
            (semantics, version) => unreachable!("no default cost model for {semantics:?} and {version:?}"),
        };

        Self::new(plutus_version, protocol_version, costs)
    }

    pub fn v1() -> Self {
        Self::default_for(PlutusVersion::V1, protocol_version::DEFAULT)
    }

    pub fn v2() -> Self {
        Self::default_for(PlutusVersion::V2, protocol_version::DEFAULT)
    }

    pub fn v3() -> Self {
        Self::default_for(PlutusVersion::V3, protocol_version::DEFAULT)
    }
}
