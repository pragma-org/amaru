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

    #[test]
    fn assert_default_cost_model_v1() {
        // 166 V1 base values, then 110 zeros for extension positions 166-275 (not read by V1
        // initialize), then ripemd_160 at 276-278, zeros for exp_mod_integer at 279-283
        // (hardcoded), and drop_list/length_of_array/list_to_array/index_array at 284-294.
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
        costs.extend(vec![0i64; 110]); // positions 166-275: extension keys not read by V1
        costs.extend([1964219, 24520, 3]); // 276-278: ripemd_160 cpu-intercept, cpu-slope, memory
        costs.extend(vec![0i64; 5]); // 279-283: exp_mod_integer (hardcoded in initialize)
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
        // initialize), then ripemd_160 at 276-278, zeros for exp_mod_integer at 279-283
        // (hardcoded), and drop_list/length_of_array/list_to_array/index_array at 284-294.
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
        costs.extend(vec![0i64; 101]); // positions 175-275: extension keys not read by V2
        costs.extend([1964219, 24520, 3]); // 276-278: ripemd_160 cpu-intercept, cpu-slope, memory
        costs.extend(vec![0i64; 5]); // 279-283: exp_mod_integer (hardcoded in initialize)
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
        let costs: Vec<i64> = vec![
            100788, 420, 1, 1, 1000, 173, 0, 1, 1000, 59957, 4, 1, 11183, 32, 201305, 8356, 4, 16000, 100, 16000, 100,
            16000, 100, 16000, 100, 16000, 100, 16000, 100, 100, 100, 16000, 100, 94375, 32, 132994, 32, 61462, 4,
            72010, 178, 0, 1, 22151, 32, 91189, 769, 4, 2, 85848, 123203, 7305, -900, 1716, 960, 57, 85848, 0, 1, 1,
            1000, 42921, 4, 2, 30623, 28755, 75, 1, 898148, 27279, 1, 51775, 558, 1, 39184, 1000, 60594, 1, 141895, 32,
            83150, 32, 15299, 32, 76049, 1, 13169, 4, 22100, 10, 28999, 74, 1, 28999, 74, 1, 43285, 552, 1, 44749, 541,
            1, 33852, 32, 68246, 32, 72362, 32, 7243, 32, 7391, 32, 11546, 32, 85848, 123203, 7305, -900, 1716, 960,
            57, 85848, 0, 1, 90434, 519, 0, 1, 74433, 32, 85848, 123203, 7305, -900, 1716, 960, 57, 85848, 0, 1, 1,
            85848, 123203, 7305, -900, 1716, 960, 57, 85848, 0, 1, 955506, 213312, 0, 2, 270652, 22588, 4, 1457325,
            64566, 4, 20467, 1, 4, 0, 141992, 32, 100788, 420, 1, 1, 81663, 32, 59498, 32, 20142, 32, 24588, 32, 20744,
            32, 25933, 32, 24623, 32, 43053543, 10, 53384111, 14333, 10, 43574283, 26308, 10, 16000, 100, 16000, 100,
            962335, 18, 2780678, 6, 442008, 1, 52538055, 3756, 18, 267929, 18, 76433006, 8868, 18, 52948122, 18,
            1995836, 36, 3227919, 12, 901022, 1, 166917843, 4307, 36, 284546, 36, 158221314, 26549, 36, 74698472, 36,
            333849714, 1, 254006273, 72, 2174038, 72, 2261318, 64571, 4, 207616, 8310, 4, 1293828, 28716, 63, 0, 1,
            1006041, 43623, 251, 0, 1, 100181, 726, 719, 0, 1, 100181, 726, 719, 0, 1, 100181, 726, 719, 0, 1, 107878,
            680, 0, 1, 95336, 1, 281145, 18848, 0, 1, 180194, 159, 1, 1, 158519, 8942, 0, 1, 159378, 8813, 0, 1,
            107490, 3298, 1, 106057, 655, 1, 1964219, 24520, 3,
        ];

        let cost_model = CostMap::new(&PlutusVersion::V3, (10, 0), &costs);

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
        costs_pv11.extend(vec![0i64; 5]);
        costs_pv11.extend([116711, 1957, 4]);
        costs_pv11.extend([198994, 10]);
        costs_pv11.extend([307802, 8496, 7, 1]);
        costs_pv11.extend([194922, 32]);
        assert_eq!(costs_pv11.len(), 295);

        let cm10 = CostMap::new(&PlutusVersion::V1, (10, 0), &costs_pv10);
        let bc10 = BuiltinCostsV1::initialize(&cm10);

        let cm11 = CostMap::new(&PlutusVersion::V1, (11, 0), &costs_pv11);
        let bc11 = BuiltinCostsV1::initialize(&cm11);

        let cost_pv10 = bc10.get_cost(DefaultFunction::ExpModInteger, &[2, 2, 2]).unwrap();
        let cost_pv11 = bc11.get_cost(DefaultFunction::ExpModInteger, &[2, 2, 2]).unwrap();

        assert_eq!(cost_pv10, cost_pv11);
        let default_cost = BuiltinCostsV1::default().get_cost(DefaultFunction::ExpModInteger, &[2, 2, 2]).unwrap();
        assert_eq!(cost_pv10, default_cost);
    }

    #[test]
    fn exp_mod_integer_cost_is_hardcoded_v3() {
        let costs = default_v3_cost_model();
        assert_eq!(costs.len(), 297);

        let cm10 = CostMap::new(&PlutusVersion::V3, (10, 0), &costs);
        let bc10 = BuiltinCostsV3::initialize(&cm10);

        let cm11 = CostMap::new(&PlutusVersion::V3, (11, 0), &costs);
        let bc11 = BuiltinCostsV3::initialize(&cm11);

        let cost_pv10 = bc10.get_cost(DefaultFunction::ExpModInteger, &[2, 2, 2]).unwrap();
        let cost_pv11 = bc11.get_cost(DefaultFunction::ExpModInteger, &[2, 2, 2]).unwrap();

        assert_eq!(cost_pv10, cost_pv11);
        let default_cost = BuiltinCostsV3::default().get_cost(DefaultFunction::ExpModInteger, &[2, 2, 2]).unwrap();
        assert_eq!(cost_pv10, default_cost);
    }
}
