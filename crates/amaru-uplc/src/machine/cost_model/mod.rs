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

pub mod builtin_costs;
pub(crate) mod cost_map;
mod costing;
pub mod ex_budget;
mod machine_costs;
mod value;

pub use value::*;

use crate::machine::{
    ExBudget, PlutusVersion,
    cost_model::{builtin_costs::BuiltinCostModel, machine_costs::MachineCosts},
};

#[derive(Debug, PartialEq)]
pub struct CostModel<B: BuiltinCostModel> {
    pub machine_startup: ExBudget,
    pub machine_costs: MachineCosts,
    pub builtin_costs: B,
}

impl<B: BuiltinCostModel> CostModel<B> {
    pub fn initialize_cost_model(
        version: &PlutusVersion,
        protocol_version: (u64, u64),
        cost_model: &[i64],
    ) -> CostModel<B> {
        let cost_map = cost_map::CostMap::new(version, protocol_version, cost_model);
        Self {
            machine_startup: ExBudget {
                mem: cost_map["cek_startup_cost-exBudgetmem"],
                cpu: cost_map["cek_startup_cost-exBudgetCPU"],
            },
            machine_costs: MachineCosts::initialize_machine_costs(&cost_map),
            builtin_costs: B::initialize(&cost_map),
        }
    }
}

impl<B: BuiltinCostModel + Default> Default for CostModel<B> {
    fn default() -> Self {
        Self {
            machine_startup: ExBudget::start_up(),
            machine_costs: Default::default(),
            builtin_costs: Default::default(),
        }
    }
}

/// Default V3 cost model values (251 base + 46 PLOMIN = 297 entries).
/// These match the Cardano mainnet default cost parameters for PlutusV3.
pub fn default_v3_cost_model() -> Vec<i64> {
    vec![
        100788, 420, 1, 1, 1000, 173, 0, 1, 1000, 59957, 4, 1, 11183, 32, 201305, 8356, 4, 16000, 100, 16000, 100,
        16000, 100, 16000, 100, 16000, 100, 16000, 100, 100, 100, 16000, 100, 94375, 32, 132994, 32, 61462, 4, 72010,
        178, 0, 1, 22151, 32, 91189, 769, 4, 2, 85848, 123203, 7305, -900, 1716, 960, 57, 85848, 0, 1, 1, 1000, 42921,
        4, 2, 30623, 28755, 75, 1, 898148, 27279, 1, 51775, 558, 1, 39184, 1000, 60594, 1, 141895, 32, 83150, 32,
        15299, 32, 76049, 1, 13169, 4, 22100, 10, 28999, 74, 1, 28999, 74, 1, 43285, 552, 1, 44749, 541, 1, 33852, 32,
        68246, 32, 72362, 32, 7243, 32, 7391, 32, 11546, 32, 85848, 123203, 7305, -900, 1716, 960, 57, 85848, 0, 1,
        90434, 519, 0, 1, 74433, 32, 85848, 123203, 7305, -900, 1716, 960, 57, 85848, 0, 1, 1, 85848, 123203, 7305,
        -900, 1716, 960, 57, 85848, 0, 1, 955506, 213312, 0, 2, 270652, 22588, 4, 1457325, 64566, 4, 20467, 1, 4, 0,
        141992, 32, 100788, 420, 1, 1, 81663, 32, 59498, 32, 20142, 32, 24588, 32, 20744, 32, 25933, 32, 24623, 32,
        43053543, 10, 53384111, 14333, 10, 43574283, 26308, 10, 16000, 100, 16000, 100, 962335, 18, 2780678, 6, 442008,
        1, 52538055, 3756, 18, 267929, 18, 76433006, 8868, 18, 52948122, 18, 1995836, 36, 3227919, 12, 901022, 1,
        166917843, 4307, 36, 284546, 36, 158221314, 26549, 36, 74698472, 36, 333849714, 1, 254006273, 72, 2174038, 72,
        2261318, 64571, 4, 207616, 8310, 4, 1293828, 28716, 63, 0, 1, 1006041, 43623, 251, 0, 1, 100181, 726, 719, 0,
        1, 100181, 726, 719, 0, 1, 100181, 726, 719, 0, 1, 107878, 680, 0, 1, 95336, 1, 281145, 18848, 0, 1, 180194,
        159, 1, 1, 158519, 8942, 0, 1, 159378, 8813, 0, 1, 107490, 3298, 1, 106057, 655, 1, 1964219, 24520, 3,
    ]
}

#[repr(usize)]
pub enum StepKind {
    Constant = 0,
    Var = 1,
    Lambda = 2,
    Apply = 3,
    Delay = 4,
    Force = 5,
    Builtin = 6,
    Constr = 7,
    Case = 8,
}
