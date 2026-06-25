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

pub mod builtin_costs_v1;
pub mod builtin_costs_v2;
pub mod builtin_costs_v3;

use crate::{
    builtin::DefaultFunction,
    machine::{ExBudget, cost_model::cost_map::CostMap},
};

pub trait BuiltinCostModel {
    fn initialize(cost_map: &CostMap) -> Self;
    fn get_cost(&self, builtin: DefaultFunction, args: &[i64]) -> Option<ExBudget>;
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;
    use crate::{
        builtin::DefaultFunction,
        machine::{
            ExBudget, PlutusVersion,
            cost_model::{
                builtin_costs::{
                    builtin_costs_v1::BuiltinCostsV1, builtin_costs_v2::BuiltinCostsV2,
                    builtin_costs_v3::BuiltinCostsV3,
                },
                default_v3_cost_model,
            },
        },
    };

    fn default_pv11_cost_values() -> Vec<i64> {
        vec![
            607153, 231697, 53144, 0, 1, 116711, 1957, 4, 231883, 10, 1000, 24838, 7, 1, 232010, 32, 321837444,
            25087669, 18, 617887431, 67302824, 36, 356924, 18413, 45, 21, 219951, 9444, 1, 1000, 172116, 183150, 6, 24,
            21, 213283, 618401, 1998, 28258, 1, 1000, 38159, 2, 22, 1000, 95933, 1, 1, 11, 1000, 277577, 12, 21,
        ]
    }

    #[test]
    fn assert_default_cost_model_v1() {
        // 166 V1 base values, then 110 zeros for extension positions 166-275 (not read by V1
        // initialize), then ripemd_160 at 276-278, exp_mod_integer at 279-283,
        // and drop_list/length_of_array/list_to_array/index_array at 284-294.
        let mut costs: Vec<i64> = vec![
            100788, 420, 1, 1, 1000, 173, 0, 1, 1000, 59957, 4, 1, 11183, 32, 201305, 8356, 4, 16000, 100, 16000, 100,
            16000, 100, 16000, 100, 16000, 100, 16000, 100, 100, 100, 16000, 100, 94375, 32, 132994, 32, 61462, 4,
            72010, 178, 0, 1, 22151, 32, 91189, 769, 4, 2, 85848, 228465, 122, 0, 1, 1, 1000, 42921, 4, 2, 24548,
            29498, 38, 1, 898148, 27279, 1, 51775, 558, 1, 39184, 1000, 60594, 1, 141895, 32, 83150, 32, 15299, 32,
            76049, 1, 13169, 4, 22100, 10, 28999, 74, 1, 28999, 74, 1, 43285, 552, 1, 44749, 541, 1, 33852, 32, 68246,
            32, 72362, 32, 7243, 32, 7391, 32, 11546, 32, 85848, 228465, 122, 0, 1, 1, 90434, 519, 0, 1, 74433, 32,
            85848, 228465, 122, 0, 1, 1, 85848, 228465, 122, 0, 1, 1, 270652, 22588, 4, 1457325, 64566, 4, 20467, 1, 4,
            0, 141992, 32, 100788, 420, 1, 1, 81663, 32, 59498, 32, 20142, 32, 24588, 32, 20744, 32, 25933, 32, 24623,
            32, 53384111, 14333, 10,
        ];
        assert_eq!(costs.len(), 166);
        costs.extend(vec![0i64; 57]); // positions 166-222: extension keys not read by V1
        costs.extend([1293828, 28716, 63, 0, 1]); // 223-227: integerToByteString
        costs.extend([1006041, 43623, 251, 0, 1]); // 228-232: byteStringToInteger
        costs.extend(vec![0i64; 43]); // positions 233-275
        costs.extend([1964219, 24520, 3]); // 276-278: ripemd_160 cpu-intercept, cpu-slope, memory
        costs.extend([607153, 231697, 53144, 0, 1]); // 279-283: exp_mod_integer
        costs.extend([116711, 1957, 4]); // 284-286: drop_list cpu-intercept, cpu-slope, mem
        costs.extend([198994, 10]); // 287-288: length_of_array cpu, mem
        costs.extend([307802, 8496, 7, 1]); // 289-292: list_to_array cpu-intercept, cpu-slope, mem-intercept, mem-slope
        costs.extend([194922, 32]); // 293-294: index_array cpu, mem
        assert_eq!(costs.len(), 295);

        let cost_model = CostMap::new(&PlutusVersion::V1, (11, 0), &costs);

        assert_eq!(BuiltinCostsV1::default(), BuiltinCostsV1::initialize(&cost_model));
    }

    #[test]
    fn assert_default_cost_model_v2() {
        // 175 V2 base values, then 101 zeros for extension positions 175-275 (not read by V2
        // initialize), then ripemd_160 at 276-278, exp_mod_integer at 279-283,
        // and drop_list/length_of_array/list_to_array/index_array at 284-294.
        let mut costs: Vec<i64> = vec![
            100788, 420, 1, 1, 1000, 173, 0, 1, 1000, 59957, 4, 1, 11183, 32, 201305, 8356, 4, 16000, 100, 16000, 100,
            16000, 100, 16000, 100, 16000, 100, 16000, 100, 100, 100, 16000, 100, 94375, 32, 132994, 32, 61462, 4,
            72010, 178, 0, 1, 22151, 32, 91189, 769, 4, 2, 85848, 228465, 122, 0, 1, 1, 1000, 42921, 4, 2, 24548,
            29498, 38, 1, 898148, 27279, 1, 51775, 558, 1, 39184, 1000, 60594, 1, 141895, 32, 83150, 32, 15299, 32,
            76049, 1, 13169, 4, 22100, 10, 28999, 74, 1, 28999, 74, 1, 43285, 552, 1, 44749, 541, 1, 33852, 32, 68246,
            32, 72362, 32, 7243, 32, 7391, 32, 11546, 32, 85848, 228465, 122, 0, 1, 1, 90434, 519, 0, 1, 74433, 32,
            85848, 228465, 122, 0, 1, 1, 85848, 228465, 122, 0, 1, 1, 955506, 213312, 0, 2, 270652, 22588, 4, 1457325,
            64566, 4, 20467, 1, 4, 0, 141992, 32, 100788, 420, 1, 1, 81663, 32, 59498, 32, 20142, 32, 24588, 32, 20744,
            32, 25933, 32, 24623, 32, 43053543, 10, 53384111, 14333, 10, 43574283, 26308, 10,
        ];
        assert_eq!(costs.len(), 175);
        costs.extend([1293828, 28716, 63, 0, 1]); // 175-179: integerToByteString
        costs.extend([1006041, 43623, 251, 0, 1]); // 180-184: byteStringToInteger
        costs.extend(vec![0i64; 91]); // positions 185-275: extension keys not read by V2
        costs.extend([1964219, 24520, 3]); // 276-278: ripemd_160 cpu-intercept, cpu-slope, memory
        costs.extend([607153, 231697, 53144, 0, 1]); // 279-283: exp_mod_integer
        costs.extend([116711, 1957, 4]); // 284-286: drop_list cpu-intercept, cpu-slope, mem
        costs.extend([198994, 10]); // 287-288: length_of_array cpu, mem
        costs.extend([307802, 8496, 7, 1]); // 289-292: list_to_array cpu-intercept, cpu-slope, mem-intercept, mem-slope
        costs.extend([194922, 32]); // 293-294: index_array cpu, mem
        assert_eq!(costs.len(), 295);

        let cost_model = CostMap::new(&PlutusVersion::V2, (11, 0), &costs);

        assert_eq!(BuiltinCostsV2::default(), BuiltinCostsV2::initialize(&cost_model));
    }

    #[test]
    fn assert_default_cost_model_v3() {
        let mut costs: Vec<i64> = default_v3_cost_model();
        assert_eq!(costs.len(), 297);
        costs.extend(default_pv11_cost_values());

        let cost_model = CostMap::new(&PlutusVersion::V3, (11, 0), &costs);

        assert_eq!(BuiltinCostsV3::default(), BuiltinCostsV3::initialize(&cost_model));
    }

    // Pre-Plomin V1 (166 entries): ripemd_160 keys absent from the map, so initialize()
    // falls back to the sentinel value (30_000_000_000) for all three ripemd_160 params.
    #[test]
    fn ripemd_160_cost_pre_plomin_v1() {
        let costs = vec![
            100788, 420, 1, 1, 1000, 173, 0, 1, 1000, 59957, 4, 1, 11183, 32, 201305, 8356, 4, 16000, 100, 16000, 100,
            16000, 100, 16000, 100, 16000, 100, 16000, 100, 100, 100, 16000, 100, 94375, 32, 132994, 32, 61462, 4,
            72010, 178, 0, 1, 22151, 32, 91189, 769, 4, 2, 85848, 228465, 122, 0, 1, 1, 1000, 42921, 4, 2, 24548,
            29498, 38, 1, 898148, 27279, 1, 51775, 558, 1, 39184, 1000, 60594, 1, 141895, 32, 83150, 32, 15299, 32,
            76049, 1, 13169, 4, 22100, 10, 28999, 74, 1, 28999, 74, 1, 43285, 552, 1, 44749, 541, 1, 33852, 32, 68246,
            32, 72362, 32, 7243, 32, 7391, 32, 11546, 32, 85848, 228465, 122, 0, 1, 1, 90434, 519, 0, 1, 74433, 32,
            85848, 228465, 122, 0, 1, 1, 85848, 228465, 122, 0, 1, 1, 270652, 22588, 4, 1457325, 64566, 4, 20467, 1, 4,
            0, 141992, 32, 100788, 420, 1, 1, 81663, 32, 59498, 32, 20142, 32, 24588, 32, 20744, 32, 25933, 32, 24623,
            32, 53384111, 14333, 10,
        ];
        assert_eq!(costs.len(), 166);

        let cost_model = CostMap::new(&PlutusVersion::V1, (9, 0), &costs);
        let builtin_costs = BuiltinCostsV1::initialize(&cost_model);

        const SENTINEL: i64 = 30_000_000_000;
        let budget = builtin_costs.get_cost(DefaultFunction::Ripemd_160, &[32]).unwrap();
        // mem = constant_cost(SENTINEL), cpu = SENTINEL + SENTINEL * 32
        assert_eq!(budget, ExBudget::new(SENTINEL, SENTINEL + SENTINEL * 32));
    }

    // Post-Plomin V1 (279 entries): ripemd_160 at positions 276-278; costs parsed correctly.
    #[test]
    fn ripemd_160_cost_post_plomin_v1() {
        let mut costs: Vec<i64> = vec![
            100788, 420, 1, 1, 1000, 173, 0, 1, 1000, 59957, 4, 1, 11183, 32, 201305, 8356, 4, 16000, 100, 16000, 100,
            16000, 100, 16000, 100, 16000, 100, 16000, 100, 100, 100, 16000, 100, 94375, 32, 132994, 32, 61462, 4,
            72010, 178, 0, 1, 22151, 32, 91189, 769, 4, 2, 85848, 228465, 122, 0, 1, 1, 1000, 42921, 4, 2, 24548,
            29498, 38, 1, 898148, 27279, 1, 51775, 558, 1, 39184, 1000, 60594, 1, 141895, 32, 83150, 32, 15299, 32,
            76049, 1, 13169, 4, 22100, 10, 28999, 74, 1, 28999, 74, 1, 43285, 552, 1, 44749, 541, 1, 33852, 32, 68246,
            32, 72362, 32, 7243, 32, 7391, 32, 11546, 32, 85848, 228465, 122, 0, 1, 1, 90434, 519, 0, 1, 74433, 32,
            85848, 228465, 122, 0, 1, 1, 85848, 228465, 122, 0, 1, 1, 270652, 22588, 4, 1457325, 64566, 4, 20467, 1, 4,
            0, 141992, 32, 100788, 420, 1, 1, 81663, 32, 59498, 32, 20142, 32, 24588, 32, 20744, 32, 25933, 32, 24623,
            32, 53384111, 14333, 10,
        ];
        assert_eq!(costs.len(), 166);
        costs.extend(vec![0i64; 110]); // positions 166-275
        costs.extend([1964219, 24520, 3]); // 276-278: ripemd_160
        assert_eq!(costs.len(), 279);

        let cost_model = CostMap::new(&PlutusVersion::V1, (10, 0), &costs);
        let builtin_costs = BuiltinCostsV1::initialize(&cost_model);

        let budget = builtin_costs.get_cost(DefaultFunction::Ripemd_160, &[32]).unwrap();
        // mem = constant_cost(3), cpu = 1964219 + 24520 * 32
        assert_eq!(budget, ExBudget::new(3, 1964219 + 24520 * 32));
    }

    // Pre-Plomin V2 (175 entries): ripemd_160 keys absent; falls back to sentinel.
    #[test]
    fn ripemd_160_cost_pre_plomin_v2() {
        let costs = vec![
            100788, 420, 1, 1, 1000, 173, 0, 1, 1000, 59957, 4, 1, 11183, 32, 201305, 8356, 4, 16000, 100, 16000, 100,
            16000, 100, 16000, 100, 16000, 100, 16000, 100, 100, 100, 16000, 100, 94375, 32, 132994, 32, 61462, 4,
            72010, 178, 0, 1, 22151, 32, 91189, 769, 4, 2, 85848, 228465, 122, 0, 1, 1, 1000, 42921, 4, 2, 24548,
            29498, 38, 1, 898148, 27279, 1, 51775, 558, 1, 39184, 1000, 60594, 1, 141895, 32, 83150, 32, 15299, 32,
            76049, 1, 13169, 4, 22100, 10, 28999, 74, 1, 28999, 74, 1, 43285, 552, 1, 44749, 541, 1, 33852, 32, 68246,
            32, 72362, 32, 7243, 32, 7391, 32, 11546, 32, 85848, 228465, 122, 0, 1, 1, 90434, 519, 0, 1, 74433, 32,
            85848, 228465, 122, 0, 1, 1, 85848, 228465, 122, 0, 1, 1, 955506, 213312, 0, 2, 270652, 22588, 4, 1457325,
            64566, 4, 20467, 1, 4, 0, 141992, 32, 100788, 420, 1, 1, 81663, 32, 59498, 32, 20142, 32, 24588, 32, 20744,
            32, 25933, 32, 24623, 32, 43053543, 10, 53384111, 14333, 10, 43574283, 26308, 10,
        ];
        assert_eq!(costs.len(), 175);

        let cost_model = CostMap::new(&PlutusVersion::V2, (9, 0), &costs);
        let builtin_costs = BuiltinCostsV2::initialize(&cost_model);

        const SENTINEL: i64 = 30_000_000_000;
        let budget = builtin_costs.get_cost(DefaultFunction::Ripemd_160, &[32]).unwrap();
        assert_eq!(budget, ExBudget::new(SENTINEL, SENTINEL + SENTINEL * 32));
    }

    // Post-Plomin V2 (279 entries): ripemd_160 at positions 276-278; costs parsed correctly.
    #[test]
    fn ripemd_160_cost_post_plomin_v2() {
        let mut costs: Vec<i64> = vec![
            100788, 420, 1, 1, 1000, 173, 0, 1, 1000, 59957, 4, 1, 11183, 32, 201305, 8356, 4, 16000, 100, 16000, 100,
            16000, 100, 16000, 100, 16000, 100, 16000, 100, 100, 100, 16000, 100, 94375, 32, 132994, 32, 61462, 4,
            72010, 178, 0, 1, 22151, 32, 91189, 769, 4, 2, 85848, 228465, 122, 0, 1, 1, 1000, 42921, 4, 2, 24548,
            29498, 38, 1, 898148, 27279, 1, 51775, 558, 1, 39184, 1000, 60594, 1, 141895, 32, 83150, 32, 15299, 32,
            76049, 1, 13169, 4, 22100, 10, 28999, 74, 1, 28999, 74, 1, 43285, 552, 1, 44749, 541, 1, 33852, 32, 68246,
            32, 72362, 32, 7243, 32, 7391, 32, 11546, 32, 85848, 228465, 122, 0, 1, 1, 90434, 519, 0, 1, 74433, 32,
            85848, 228465, 122, 0, 1, 1, 85848, 228465, 122, 0, 1, 1, 955506, 213312, 0, 2, 270652, 22588, 4, 1457325,
            64566, 4, 20467, 1, 4, 0, 141992, 32, 100788, 420, 1, 1, 81663, 32, 59498, 32, 20142, 32, 24588, 32, 20744,
            32, 25933, 32, 24623, 32, 43053543, 10, 53384111, 14333, 10, 43574283, 26308, 10,
        ];
        assert_eq!(costs.len(), 175);
        costs.extend(vec![0i64; 101]); // positions 175-275
        costs.extend([1964219, 24520, 3]); // 276-278: ripemd_160
        assert_eq!(costs.len(), 279);

        let cost_model = CostMap::new(&PlutusVersion::V2, (10, 0), &costs);
        let builtin_costs = BuiltinCostsV2::initialize(&cost_model);

        let budget = builtin_costs.get_cost(DefaultFunction::Ripemd_160, &[32]).unwrap();
        assert_eq!(budget, ExBudget::new(3, 1964219 + 24520 * 32));
    }

    #[test]
    fn drop_list_cost_pre_pv11_v1() {
        let mut costs: Vec<i64> = vec![
            100788, 420, 1, 1, 1000, 173, 0, 1, 1000, 59957, 4, 1, 11183, 32, 201305, 8356, 4, 16000, 100, 16000, 100,
            16000, 100, 16000, 100, 16000, 100, 16000, 100, 100, 100, 16000, 100, 94375, 32, 132994, 32, 61462, 4,
            72010, 178, 0, 1, 22151, 32, 91189, 769, 4, 2, 85848, 228465, 122, 0, 1, 1, 1000, 42921, 4, 2, 24548,
            29498, 38, 1, 898148, 27279, 1, 51775, 558, 1, 39184, 1000, 60594, 1, 141895, 32, 83150, 32, 15299, 32,
            76049, 1, 13169, 4, 22100, 10, 28999, 74, 1, 28999, 74, 1, 43285, 552, 1, 44749, 541, 1, 33852, 32, 68246,
            32, 72362, 32, 7243, 32, 7391, 32, 11546, 32, 85848, 228465, 122, 0, 1, 1, 90434, 519, 0, 1, 74433, 32,
            85848, 228465, 122, 0, 1, 1, 85848, 228465, 122, 0, 1, 1, 270652, 22588, 4, 1457325, 64566, 4, 20467, 1, 4,
            0, 141992, 32, 100788, 420, 1, 1, 81663, 32, 59498, 32, 20142, 32, 24588, 32, 20744, 32, 25933, 32, 24623,
            32, 53384111, 14333, 10,
        ];
        assert_eq!(costs.len(), 166);
        costs.extend(vec![0i64; 110]);
        costs.extend([1964219, 24520, 3]);
        assert_eq!(costs.len(), 279);

        let cost_model = CostMap::new(&PlutusVersion::V1, (10, 0), &costs);
        let builtin_costs = BuiltinCostsV1::initialize(&cost_model);

        const SENTINEL: i64 = 30_000_000_000;
        let budget = builtin_costs.get_cost(DefaultFunction::DropList, &[5, 10]).unwrap();
        assert_eq!(budget, ExBudget::new(SENTINEL, SENTINEL + SENTINEL * 5));
    }

    #[test]
    fn drop_list_cost_post_pv11_v1() {
        let mut costs: Vec<i64> = vec![
            100788, 420, 1, 1, 1000, 173, 0, 1, 1000, 59957, 4, 1, 11183, 32, 201305, 8356, 4, 16000, 100, 16000, 100,
            16000, 100, 16000, 100, 16000, 100, 16000, 100, 100, 100, 16000, 100, 94375, 32, 132994, 32, 61462, 4,
            72010, 178, 0, 1, 22151, 32, 91189, 769, 4, 2, 85848, 228465, 122, 0, 1, 1, 1000, 42921, 4, 2, 24548,
            29498, 38, 1, 898148, 27279, 1, 51775, 558, 1, 39184, 1000, 60594, 1, 141895, 32, 83150, 32, 15299, 32,
            76049, 1, 13169, 4, 22100, 10, 28999, 74, 1, 28999, 74, 1, 43285, 552, 1, 44749, 541, 1, 33852, 32, 68246,
            32, 72362, 32, 7243, 32, 7391, 32, 11546, 32, 85848, 228465, 122, 0, 1, 1, 90434, 519, 0, 1, 74433, 32,
            85848, 228465, 122, 0, 1, 1, 85848, 228465, 122, 0, 1, 1, 270652, 22588, 4, 1457325, 64566, 4, 20467, 1, 4,
            0, 141992, 32, 100788, 420, 1, 1, 81663, 32, 59498, 32, 20142, 32, 24588, 32, 20744, 32, 25933, 32, 24623,
            32, 53384111, 14333, 10,
        ];
        assert_eq!(costs.len(), 166);
        costs.extend(vec![0i64; 110]);
        costs.extend([1964219, 24520, 3]);
        costs.extend(vec![0i64; 5]);
        costs.extend([116711, 1957, 4]);
        costs.extend([198994, 10]);
        costs.extend([307802, 8496, 7, 1]);
        costs.extend([194922, 32]);
        assert_eq!(costs.len(), 295);

        let cost_model = CostMap::new(&PlutusVersion::V1, (11, 0), &costs);
        let builtin_costs = BuiltinCostsV1::initialize(&cost_model);

        let budget = builtin_costs.get_cost(DefaultFunction::DropList, &[5, 10]).unwrap();
        assert_eq!(budget, ExBudget::new(4, 116711 + 1957 * 5));
    }

    #[test]
    fn drop_list_cost_pre_pv11_v2() {
        let mut costs: Vec<i64> = vec![
            100788, 420, 1, 1, 1000, 173, 0, 1, 1000, 59957, 4, 1, 11183, 32, 201305, 8356, 4, 16000, 100, 16000, 100,
            16000, 100, 16000, 100, 16000, 100, 16000, 100, 100, 100, 16000, 100, 94375, 32, 132994, 32, 61462, 4,
            72010, 178, 0, 1, 22151, 32, 91189, 769, 4, 2, 85848, 228465, 122, 0, 1, 1, 1000, 42921, 4, 2, 24548,
            29498, 38, 1, 898148, 27279, 1, 51775, 558, 1, 39184, 1000, 60594, 1, 141895, 32, 83150, 32, 15299, 32,
            76049, 1, 13169, 4, 22100, 10, 28999, 74, 1, 28999, 74, 1, 43285, 552, 1, 44749, 541, 1, 33852, 32, 68246,
            32, 72362, 32, 7243, 32, 7391, 32, 11546, 32, 85848, 228465, 122, 0, 1, 1, 90434, 519, 0, 1, 74433, 32,
            85848, 228465, 122, 0, 1, 1, 85848, 228465, 122, 0, 1, 1, 955506, 213312, 0, 2, 270652, 22588, 4, 1457325,
            64566, 4, 20467, 1, 4, 0, 141992, 32, 100788, 420, 1, 1, 81663, 32, 59498, 32, 20142, 32, 24588, 32, 20744,
            32, 25933, 32, 24623, 32, 43053543, 10, 53384111, 14333, 10, 43574283, 26308, 10,
        ];
        assert_eq!(costs.len(), 175);
        costs.extend(vec![0i64; 101]);
        costs.extend([1964219, 24520, 3]);
        assert_eq!(costs.len(), 279);

        let cost_model = CostMap::new(&PlutusVersion::V2, (10, 0), &costs);
        let builtin_costs = BuiltinCostsV2::initialize(&cost_model);

        const SENTINEL: i64 = 30_000_000_000;
        let budget = builtin_costs.get_cost(DefaultFunction::DropList, &[5, 10]).unwrap();
        assert_eq!(budget, ExBudget::new(SENTINEL, SENTINEL + SENTINEL * 5));
    }

    #[test]
    fn drop_list_cost_post_pv11_v2() {
        let mut costs: Vec<i64> = vec![
            100788, 420, 1, 1, 1000, 173, 0, 1, 1000, 59957, 4, 1, 11183, 32, 201305, 8356, 4, 16000, 100, 16000, 100,
            16000, 100, 16000, 100, 16000, 100, 16000, 100, 100, 100, 16000, 100, 94375, 32, 132994, 32, 61462, 4,
            72010, 178, 0, 1, 22151, 32, 91189, 769, 4, 2, 85848, 228465, 122, 0, 1, 1, 1000, 42921, 4, 2, 24548,
            29498, 38, 1, 898148, 27279, 1, 51775, 558, 1, 39184, 1000, 60594, 1, 141895, 32, 83150, 32, 15299, 32,
            76049, 1, 13169, 4, 22100, 10, 28999, 74, 1, 28999, 74, 1, 43285, 552, 1, 44749, 541, 1, 33852, 32, 68246,
            32, 72362, 32, 7243, 32, 7391, 32, 11546, 32, 85848, 228465, 122, 0, 1, 1, 90434, 519, 0, 1, 74433, 32,
            85848, 228465, 122, 0, 1, 1, 85848, 228465, 122, 0, 1, 1, 955506, 213312, 0, 2, 270652, 22588, 4, 1457325,
            64566, 4, 20467, 1, 4, 0, 141992, 32, 100788, 420, 1, 1, 81663, 32, 59498, 32, 20142, 32, 24588, 32, 20744,
            32, 25933, 32, 24623, 32, 43053543, 10, 53384111, 14333, 10, 43574283, 26308, 10,
        ];
        assert_eq!(costs.len(), 175);
        costs.extend(vec![0i64; 101]);
        costs.extend([1964219, 24520, 3]);
        costs.extend(vec![0i64; 5]);
        costs.extend([116711, 1957, 4]);
        costs.extend([198994, 10]);
        costs.extend([307802, 8496, 7, 1]);
        costs.extend([194922, 32]);
        assert_eq!(costs.len(), 295);

        let cost_model = CostMap::new(&PlutusVersion::V2, (11, 0), &costs);
        let builtin_costs = BuiltinCostsV2::initialize(&cost_model);

        let budget = builtin_costs.get_cost(DefaultFunction::DropList, &[5, 10]).unwrap();
        assert_eq!(budget, ExBudget::new(4, 116711 + 1957 * 5));
    }

    const ALL_BUILTINS: &[DefaultFunction] = &[
        DefaultFunction::AddInteger,
        DefaultFunction::SubtractInteger,
        DefaultFunction::MultiplyInteger,
        DefaultFunction::DivideInteger,
        DefaultFunction::QuotientInteger,
        DefaultFunction::RemainderInteger,
        DefaultFunction::ModInteger,
        DefaultFunction::EqualsInteger,
        DefaultFunction::LessThanInteger,
        DefaultFunction::LessThanEqualsInteger,
        DefaultFunction::AppendByteString,
        DefaultFunction::ConsByteString,
        DefaultFunction::SliceByteString,
        DefaultFunction::LengthOfByteString,
        DefaultFunction::IndexByteString,
        DefaultFunction::EqualsByteString,
        DefaultFunction::LessThanByteString,
        DefaultFunction::LessThanEqualsByteString,
        DefaultFunction::Sha2_256,
        DefaultFunction::Sha3_256,
        DefaultFunction::Blake2b_256,
        DefaultFunction::Keccak_256,
        DefaultFunction::Blake2b_224,
        DefaultFunction::VerifyEd25519Signature,
        DefaultFunction::VerifyEcdsaSecp256k1Signature,
        DefaultFunction::VerifySchnorrSecp256k1Signature,
        DefaultFunction::AppendString,
        DefaultFunction::EqualsString,
        DefaultFunction::EncodeUtf8,
        DefaultFunction::DecodeUtf8,
        DefaultFunction::IfThenElse,
        DefaultFunction::ChooseUnit,
        DefaultFunction::Trace,
        DefaultFunction::FstPair,
        DefaultFunction::SndPair,
        DefaultFunction::ChooseList,
        DefaultFunction::MkCons,
        DefaultFunction::HeadList,
        DefaultFunction::TailList,
        DefaultFunction::NullList,
        DefaultFunction::ChooseData,
        DefaultFunction::ConstrData,
        DefaultFunction::MapData,
        DefaultFunction::ListData,
        DefaultFunction::IData,
        DefaultFunction::BData,
        DefaultFunction::UnConstrData,
        DefaultFunction::UnMapData,
        DefaultFunction::UnListData,
        DefaultFunction::UnIData,
        DefaultFunction::UnBData,
        DefaultFunction::EqualsData,
        DefaultFunction::SerialiseData,
        DefaultFunction::MkPairData,
        DefaultFunction::MkNilData,
        DefaultFunction::MkNilPairData,
        DefaultFunction::Bls12_381_G1_Add,
        DefaultFunction::Bls12_381_G1_Neg,
        DefaultFunction::Bls12_381_G1_ScalarMul,
        DefaultFunction::Bls12_381_G1_Equal,
        DefaultFunction::Bls12_381_G1_Compress,
        DefaultFunction::Bls12_381_G1_Uncompress,
        DefaultFunction::Bls12_381_G1_HashToGroup,
        DefaultFunction::Bls12_381_G2_Add,
        DefaultFunction::Bls12_381_G2_Neg,
        DefaultFunction::Bls12_381_G2_ScalarMul,
        DefaultFunction::Bls12_381_G2_Equal,
        DefaultFunction::Bls12_381_G2_Compress,
        DefaultFunction::Bls12_381_G2_Uncompress,
        DefaultFunction::Bls12_381_G2_HashToGroup,
        DefaultFunction::Bls12_381_MillerLoop,
        DefaultFunction::Bls12_381_MulMlResult,
        DefaultFunction::Bls12_381_FinalVerify,
        DefaultFunction::IntegerToByteString,
        DefaultFunction::ByteStringToInteger,
        DefaultFunction::AndByteString,
        DefaultFunction::OrByteString,
        DefaultFunction::XorByteString,
        DefaultFunction::ComplementByteString,
        DefaultFunction::ReadBit,
        DefaultFunction::WriteBits,
        DefaultFunction::ReplicateByte,
        DefaultFunction::ShiftByteString,
        DefaultFunction::RotateByteString,
        DefaultFunction::CountSetBits,
        DefaultFunction::FindFirstSetBit,
        DefaultFunction::Ripemd_160,
        DefaultFunction::ExpModInteger,
        DefaultFunction::DropList,
        DefaultFunction::LengthOfArray,
        DefaultFunction::ListToArray,
        DefaultFunction::IndexArray,
        DefaultFunction::Bls12_381_G1_MultiScalarMul,
        DefaultFunction::Bls12_381_G2_MultiScalarMul,
        DefaultFunction::InsertCoin,
        DefaultFunction::LookupCoin,
        DefaultFunction::UnionValue,
        DefaultFunction::ValueContains,
        DefaultFunction::ValueData,
        DefaultFunction::UnValueData,
        DefaultFunction::ScaleValue,
    ];

    fn assert_available_builtins_have_costs<C: BuiltinCostModel + Default>(version: PlutusVersion, pv: u32) {
        let costs = C::default();
        let dummy_args: Vec<i64> = vec![1; 6];
        for &builtin in ALL_BUILTINS {
            if builtin.is_available_in(version, pv) {
                assert!(
                    costs.get_cost(builtin, &dummy_args).is_some(),
                    "get_cost returned None for {builtin:?} which is available in {version:?} at PV {pv}"
                );
            }
        }
    }

    #[test]
    fn available_builtins_have_costs_v1() {
        for pv in 5..=10 {
            assert_available_builtins_have_costs::<BuiltinCostsV1>(PlutusVersion::V1, pv);
        }
    }

    #[test]
    fn available_builtins_have_costs_v2() {
        for pv in 7..=10 {
            assert_available_builtins_have_costs::<BuiltinCostsV2>(PlutusVersion::V2, pv);
        }
    }

    #[test]
    fn available_builtins_have_costs_v3() {
        for pv in 9..=10 {
            assert_available_builtins_have_costs::<BuiltinCostsV3>(PlutusVersion::V3, pv);
        }
    }

    #[test]
    #[ignore = "PV 11 opens all builtins for V1/V2 but cost models are not yet complete"]
    fn available_builtins_have_costs_pv11() {
        assert_available_builtins_have_costs::<BuiltinCostsV1>(PlutusVersion::V1, 11);
        assert_available_builtins_have_costs::<BuiltinCostsV2>(PlutusVersion::V2, 11);
        assert_available_builtins_have_costs::<BuiltinCostsV3>(PlutusVersion::V3, 11);
    }

    #[test]
    fn exp_mod_integer_cost_is_hardcoded_v1() {
        let mut costs_pv10: Vec<i64> = vec![
            100788, 420, 1, 1, 1000, 173, 0, 1, 1000, 59957, 4, 1, 11183, 32, 201305, 8356, 4, 16000, 100, 16000, 100,
            16000, 100, 16000, 100, 16000, 100, 16000, 100, 100, 100, 16000, 100, 94375, 32, 132994, 32, 61462, 4,
            72010, 178, 0, 1, 22151, 32, 91189, 769, 4, 2, 85848, 228465, 122, 0, 1, 1, 1000, 42921, 4, 2, 24548,
            29498, 38, 1, 898148, 27279, 1, 51775, 558, 1, 39184, 1000, 60594, 1, 141895, 32, 83150, 32, 15299, 32,
            76049, 1, 13169, 4, 22100, 10, 28999, 74, 1, 28999, 74, 1, 43285, 552, 1, 44749, 541, 1, 33852, 32, 68246,
            32, 72362, 32, 7243, 32, 7391, 32, 11546, 32, 85848, 228465, 122, 0, 1, 1, 90434, 519, 0, 1, 74433, 32,
            85848, 228465, 122, 0, 1, 1, 85848, 228465, 122, 0, 1, 1, 270652, 22588, 4, 1457325, 64566, 4, 20467, 1, 4,
            0, 141992, 32, 100788, 420, 1, 1, 81663, 32, 59498, 32, 20142, 32, 24588, 32, 20744, 32, 25933, 32, 24623,
            32, 53384111, 14333, 10,
        ];
        assert_eq!(costs_pv10.len(), 166);
        costs_pv10.extend(vec![0i64; 110]);
        costs_pv10.extend([1964219, 24520, 3]);
        assert_eq!(costs_pv10.len(), 279);

        let mut costs_pv11 = costs_pv10.clone();
        costs_pv11.extend([607153, 231697, 53144, 0, 1]);
        costs_pv11.extend([116711, 1957, 4]);
        costs_pv11.extend([198994, 10]);
        costs_pv11.extend([307802, 8496, 7, 1]);
        costs_pv11.extend([194922, 32]);
        assert_eq!(costs_pv11.len(), 295);

        let cm11 = CostMap::new(&PlutusVersion::V1, (11, 0), &costs_pv11);
        let bc11 = BuiltinCostsV1::initialize(&cm11);

        let cost_pv11 = bc11.get_cost(DefaultFunction::ExpModInteger, &[2, 2, 2]).unwrap();
        let default_cost = BuiltinCostsV1::default().get_cost(DefaultFunction::ExpModInteger, &[2, 2, 2]).unwrap();
        assert_eq!(cost_pv11, default_cost);
    }

    #[test]
    fn exp_mod_integer_cost_is_hardcoded_v3() {
        let mut costs = default_v3_cost_model();
        assert_eq!(costs.len(), 297);
        costs.extend(default_pv11_cost_values());

        let cm11 = CostMap::new(&PlutusVersion::V3, (11, 0), &costs);
        let bc11 = BuiltinCostsV3::initialize(&cm11);

        let cost_pv11 = bc11.get_cost(DefaultFunction::ExpModInteger, &[2, 2, 2]).unwrap();
        let default_cost = BuiltinCostsV3::default().get_cost(DefaultFunction::ExpModInteger, &[2, 2, 2]).unwrap();
        assert_eq!(cost_pv11, default_cost);
    }
}
