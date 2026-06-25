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

use crate::{
    builtin::DefaultFunction,
    machine::{
        ExBudget,
        cost_model::{
            builtin_costs::BuiltinCostModel,
            cost_map::CostMap,
            costing::{Cost, OneArgumentCosting, SixArgumentsCosting, ThreeArgumentsCosting, TwoArgumentsCosting},
        },
    },
};

#[derive(Debug, PartialEq)]
pub struct BuiltinCostsV1 {
    add_integer: TwoArgumentsCosting,
    subtract_integer: TwoArgumentsCosting,
    multiply_integer: TwoArgumentsCosting,
    divide_integer: TwoArgumentsCosting,
    quotient_integer: TwoArgumentsCosting,
    remainder_integer: TwoArgumentsCosting,
    mod_integer: TwoArgumentsCosting,
    equals_integer: TwoArgumentsCosting,
    less_than_integer: TwoArgumentsCosting,
    less_than_equals_integer: TwoArgumentsCosting,
    // Bytestrings
    append_byte_string: TwoArgumentsCosting,
    cons_byte_string: TwoArgumentsCosting,
    slice_byte_string: ThreeArgumentsCosting,
    length_of_byte_string: OneArgumentCosting,
    index_byte_string: TwoArgumentsCosting,
    equals_byte_string: TwoArgumentsCosting,
    less_than_byte_string: TwoArgumentsCosting,
    less_than_equals_byte_string: TwoArgumentsCosting,
    // Cryptography and hashes
    sha2_256: OneArgumentCosting,
    sha3_256: OneArgumentCosting,
    blake2b_256: OneArgumentCosting,
    verify_ed25519_signature: ThreeArgumentsCosting,
    // Strings
    append_string: TwoArgumentsCosting,
    equals_string: TwoArgumentsCosting,
    encode_utf8: OneArgumentCosting,
    decode_utf8: OneArgumentCosting,
    // Bool
    if_then_else: ThreeArgumentsCosting,
    // Unit
    choose_unit: TwoArgumentsCosting,
    // Tracing
    trace: TwoArgumentsCosting,
    // Pairs
    fst_pair: OneArgumentCosting,
    snd_pair: OneArgumentCosting,
    // Lists
    choose_list: ThreeArgumentsCosting,
    mk_cons: TwoArgumentsCosting,
    head_list: OneArgumentCosting,
    tail_list: OneArgumentCosting,
    null_list: OneArgumentCosting,
    // Data
    choose_data: SixArgumentsCosting,
    constr_data: TwoArgumentsCosting,
    map_data: OneArgumentCosting,
    list_data: OneArgumentCosting,
    i_data: OneArgumentCosting,
    b_data: OneArgumentCosting,
    un_constr_data: OneArgumentCosting,
    un_map_data: OneArgumentCosting,
    un_list_data: OneArgumentCosting,
    un_i_data: OneArgumentCosting,
    un_b_data: OneArgumentCosting,
    equals_data: TwoArgumentsCosting,
    // Misc constructors
    mk_pair_data: TwoArgumentsCosting,
    mk_nil_data: OneArgumentCosting,
    mk_nil_pair_data: OneArgumentCosting,
    // bitwise
    integer_to_byte_string: ThreeArgumentsCosting,
    byte_string_to_integer: TwoArgumentsCosting,
    ripemd_160: OneArgumentCosting,

    exp_mod_integer: ThreeArgumentsCosting,
    drop_list: TwoArgumentsCosting,
    length_of_array: OneArgumentCosting,
    list_to_array: TwoArgumentsCosting,
    index_array: TwoArgumentsCosting,
}

impl Default for BuiltinCostsV1 {
    fn default() -> Self {
        Self {
            add_integer: TwoArgumentsCosting::new(
                TwoArgumentsCosting::max_size(1, 1),
                TwoArgumentsCosting::max_size(100788, 420),
            ),
            subtract_integer: TwoArgumentsCosting::new(
                TwoArgumentsCosting::max_size(1, 1),
                TwoArgumentsCosting::max_size(100788, 420),
            ),
            multiply_integer: TwoArgumentsCosting::new(
                TwoArgumentsCosting::added_sizes(0, 1),
                TwoArgumentsCosting::multiplied_sizes(90434, 519),
            ),
            divide_integer: TwoArgumentsCosting::new(
                TwoArgumentsCosting::subtracted_sizes(0, 1, 1),
                TwoArgumentsCosting::const_above_diagonal_into_multiplied_sizes(85848, 228465, 122),
            ),
            quotient_integer: TwoArgumentsCosting::new(
                TwoArgumentsCosting::subtracted_sizes(0, 1, 1),
                TwoArgumentsCosting::const_above_diagonal_into_multiplied_sizes(85848, 228465, 122),
            ),
            remainder_integer: TwoArgumentsCosting::new(
                TwoArgumentsCosting::subtracted_sizes(0, 1, 1),
                TwoArgumentsCosting::const_above_diagonal_into_multiplied_sizes(85848, 228465, 122),
            ),
            mod_integer: TwoArgumentsCosting::new(
                TwoArgumentsCosting::subtracted_sizes(0, 1, 1),
                TwoArgumentsCosting::const_above_diagonal_into_multiplied_sizes(85848, 228465, 122),
            ),
            equals_integer: TwoArgumentsCosting::new(
                TwoArgumentsCosting::constant_cost(1),
                TwoArgumentsCosting::min_size(51775, 558),
            ),
            less_than_integer: TwoArgumentsCosting::new(
                TwoArgumentsCosting::constant_cost(1),
                TwoArgumentsCosting::min_size(44749, 541),
            ),
            less_than_equals_integer: TwoArgumentsCosting::new(
                TwoArgumentsCosting::constant_cost(1),
                TwoArgumentsCosting::min_size(43285, 552),
            ),
            append_byte_string: TwoArgumentsCosting::new(
                TwoArgumentsCosting::added_sizes(0, 1),
                TwoArgumentsCosting::added_sizes(1000, 173),
            ),
            cons_byte_string: TwoArgumentsCosting::new(
                TwoArgumentsCosting::added_sizes(0, 1),
                TwoArgumentsCosting::linear_in_y(72010, 178),
            ),
            slice_byte_string: ThreeArgumentsCosting::new(
                ThreeArgumentsCosting::linear_in_z(4, 0),
                ThreeArgumentsCosting::linear_in_z(20467, 1),
            ),
            length_of_byte_string: OneArgumentCosting::new(
                OneArgumentCosting::constant_cost(10),
                OneArgumentCosting::constant_cost(22100),
            ),
            index_byte_string: TwoArgumentsCosting::new(
                TwoArgumentsCosting::constant_cost(4),
                TwoArgumentsCosting::constant_cost(13169),
            ),
            equals_byte_string: TwoArgumentsCosting::new(
                TwoArgumentsCosting::constant_cost(1),
                TwoArgumentsCosting::linear_on_diagonal(24548, 29498, 38),
            ),
            less_than_byte_string: TwoArgumentsCosting::new(
                TwoArgumentsCosting::constant_cost(1),
                TwoArgumentsCosting::min_size(28999, 74),
            ),
            less_than_equals_byte_string: TwoArgumentsCosting::new(
                TwoArgumentsCosting::constant_cost(1),
                TwoArgumentsCosting::min_size(28999, 74),
            ),
            sha2_256: OneArgumentCosting::new(
                OneArgumentCosting::constant_cost(4),
                OneArgumentCosting::linear_cost(270652, 22588),
            ),
            sha3_256: OneArgumentCosting::new(
                OneArgumentCosting::constant_cost(4),
                OneArgumentCosting::linear_cost(1457325, 64566),
            ),
            blake2b_256: OneArgumentCosting::new(
                OneArgumentCosting::constant_cost(4),
                OneArgumentCosting::linear_cost(201305, 8356),
            ),
            verify_ed25519_signature: ThreeArgumentsCosting::new(
                ThreeArgumentsCosting::constant_cost(10),
                ThreeArgumentsCosting::linear_in_y(53384111, 14333),
            ),
            append_string: TwoArgumentsCosting::new(
                TwoArgumentsCosting::added_sizes(4, 1),
                TwoArgumentsCosting::added_sizes(1000, 59957),
            ),
            equals_string: TwoArgumentsCosting::new(
                TwoArgumentsCosting::constant_cost(1),
                TwoArgumentsCosting::linear_on_diagonal(39184, 1000, 60594),
            ),
            encode_utf8: OneArgumentCosting::new(
                OneArgumentCosting::linear_cost(4, 2),
                OneArgumentCosting::linear_cost(1000, 42921),
            ),
            decode_utf8: OneArgumentCosting::new(
                OneArgumentCosting::linear_cost(4, 2),
                OneArgumentCosting::linear_cost(91189, 769),
            ),
            if_then_else: ThreeArgumentsCosting::new(
                ThreeArgumentsCosting::constant_cost(1),
                ThreeArgumentsCosting::constant_cost(76049),
            ),
            choose_unit: TwoArgumentsCosting::new(
                TwoArgumentsCosting::constant_cost(4),
                TwoArgumentsCosting::constant_cost(61462),
            ),
            trace: TwoArgumentsCosting::new(
                TwoArgumentsCosting::constant_cost(32),
                TwoArgumentsCosting::constant_cost(59498),
            ),
            fst_pair: OneArgumentCosting::new(
                OneArgumentCosting::constant_cost(32),
                OneArgumentCosting::constant_cost(141895),
            ),
            snd_pair: OneArgumentCosting::new(
                OneArgumentCosting::constant_cost(32),
                OneArgumentCosting::constant_cost(141992),
            ),
            choose_list: ThreeArgumentsCosting::new(
                ThreeArgumentsCosting::constant_cost(32),
                ThreeArgumentsCosting::constant_cost(132994),
            ),
            mk_cons: TwoArgumentsCosting::new(
                TwoArgumentsCosting::constant_cost(32),
                TwoArgumentsCosting::constant_cost(72362),
            ),
            head_list: OneArgumentCosting::new(
                OneArgumentCosting::constant_cost(32),
                OneArgumentCosting::constant_cost(83150),
            ),
            tail_list: OneArgumentCosting::new(
                OneArgumentCosting::constant_cost(32),
                OneArgumentCosting::constant_cost(81663),
            ),
            null_list: OneArgumentCosting::new(
                OneArgumentCosting::constant_cost(32),
                OneArgumentCosting::constant_cost(74433),
            ),
            choose_data: SixArgumentsCosting::new(
                SixArgumentsCosting::constant_cost(32),
                SixArgumentsCosting::constant_cost(94375),
            ),
            constr_data: TwoArgumentsCosting::new(
                TwoArgumentsCosting::constant_cost(32),
                TwoArgumentsCosting::constant_cost(22151),
            ),
            map_data: OneArgumentCosting::new(
                OneArgumentCosting::constant_cost(32),
                OneArgumentCosting::constant_cost(68246),
            ),
            list_data: OneArgumentCosting::new(
                OneArgumentCosting::constant_cost(32),
                OneArgumentCosting::constant_cost(33852),
            ),
            i_data: OneArgumentCosting::new(
                OneArgumentCosting::constant_cost(32),
                OneArgumentCosting::constant_cost(15299),
            ),
            b_data: OneArgumentCosting::new(
                OneArgumentCosting::constant_cost(32),
                OneArgumentCosting::constant_cost(11183),
            ),
            un_constr_data: OneArgumentCosting::new(
                OneArgumentCosting::constant_cost(32),
                OneArgumentCosting::constant_cost(24588),
            ),
            un_map_data: OneArgumentCosting::new(
                OneArgumentCosting::constant_cost(32),
                OneArgumentCosting::constant_cost(24623),
            ),
            un_list_data: OneArgumentCosting::new(
                OneArgumentCosting::constant_cost(32),
                OneArgumentCosting::constant_cost(25933),
            ),
            un_i_data: OneArgumentCosting::new(
                OneArgumentCosting::constant_cost(32),
                OneArgumentCosting::constant_cost(20744),
            ),
            un_b_data: OneArgumentCosting::new(
                OneArgumentCosting::constant_cost(32),
                OneArgumentCosting::constant_cost(20142),
            ),
            equals_data: TwoArgumentsCosting::new(
                TwoArgumentsCosting::constant_cost(1),
                TwoArgumentsCosting::min_size(898148, 27279),
            ),
            mk_pair_data: TwoArgumentsCosting::new(
                TwoArgumentsCosting::constant_cost(32),
                TwoArgumentsCosting::constant_cost(11546),
            ),
            mk_nil_data: OneArgumentCosting::new(
                OneArgumentCosting::constant_cost(32),
                OneArgumentCosting::constant_cost(7243),
            ),
            mk_nil_pair_data: OneArgumentCosting::new(
                OneArgumentCosting::constant_cost(32),
                OneArgumentCosting::constant_cost(7391),
            ),
            integer_to_byte_string: ThreeArgumentsCosting::new(
                ThreeArgumentsCosting::literal_in_y_or_linear_in_z(0, 1),
                ThreeArgumentsCosting::quadratic_in_z(1293828, 28716, 63),
            ),
            byte_string_to_integer: TwoArgumentsCosting::new(
                TwoArgumentsCosting::linear_in_y(0, 1),
                TwoArgumentsCosting::quadratic_in_y(1006041, 43623, 251),
            ),
            ripemd_160: OneArgumentCosting::new(
                OneArgumentCosting::constant_cost(3),
                OneArgumentCosting::linear_cost(1964219, 24520),
            ),
            exp_mod_integer: ThreeArgumentsCosting::new(
                ThreeArgumentsCosting::linear_in_z(0, 1),
                ThreeArgumentsCosting::exp_mod_cost(607153, 231697, 53144),
            ),
            drop_list: TwoArgumentsCosting::new(
                TwoArgumentsCosting::constant_cost(4),
                TwoArgumentsCosting::linear_in_x(116711, 1957),
            ),
            length_of_array: OneArgumentCosting::new(
                OneArgumentCosting::constant_cost(10),
                OneArgumentCosting::constant_cost(198994),
            ),
            list_to_array: TwoArgumentsCosting::new(
                TwoArgumentsCosting::linear_in_x(7, 1),
                TwoArgumentsCosting::linear_in_x(307802, 8496),
            ),
            index_array: TwoArgumentsCosting::new(
                TwoArgumentsCosting::constant_cost(32),
                TwoArgumentsCosting::constant_cost(194922),
            ),
        }
    }
}

impl BuiltinCostModel for BuiltinCostsV1 {
    fn initialize(cost_map: &CostMap) -> Self {
        Self {
            add_integer: TwoArgumentsCosting::new(
                TwoArgumentsCosting::max_size(
                    cost_map["add_integer-mem-arguments-intercept"],
                    cost_map["add_integer-mem-arguments-slope"],
                ),
                TwoArgumentsCosting::max_size(
                    cost_map["add_integer-cpu-arguments-intercept"],
                    cost_map["add_integer-cpu-arguments-slope"],
                ),
            ),

            append_byte_string: TwoArgumentsCosting::new(
                TwoArgumentsCosting::added_sizes(
                    cost_map["append_byte_string-mem-arguments-intercept"],
                    cost_map["append_byte_string-mem-arguments-slope"],
                ),
                TwoArgumentsCosting::added_sizes(
                    cost_map["append_byte_string-cpu-arguments-intercept"],
                    cost_map["append_byte_string-cpu-arguments-slope"],
                ),
            ),

            append_string: TwoArgumentsCosting::new(
                TwoArgumentsCosting::added_sizes(
                    cost_map["append_string-mem-arguments-intercept"],
                    cost_map["append_string-mem-arguments-slope"],
                ),
                TwoArgumentsCosting::added_sizes(
                    cost_map["append_string-cpu-arguments-intercept"],
                    cost_map["append_string-cpu-arguments-slope"],
                ),
            ),

            b_data: OneArgumentCosting::new(
                OneArgumentCosting::constant_cost(cost_map["b_data-mem-arguments"]),
                OneArgumentCosting::constant_cost(cost_map["b_data-cpu-arguments"]),
            ),

            blake2b_256: OneArgumentCosting::new(
                OneArgumentCosting::constant_cost(cost_map["blake2b_256-mem-arguments"]),
                OneArgumentCosting::linear_cost(
                    cost_map["blake2b_256-cpu-arguments-intercept"],
                    cost_map["blake2b_256-cpu-arguments-slope"],
                ),
            ),
            choose_data: SixArgumentsCosting::new(
                SixArgumentsCosting::constant_cost(cost_map["choose_data-mem-arguments"]),
                SixArgumentsCosting::constant_cost(cost_map["choose_data-cpu-arguments"]),
            ),
            choose_list: ThreeArgumentsCosting::new(
                ThreeArgumentsCosting::constant_cost(cost_map["choose_list-mem-arguments"]),
                ThreeArgumentsCosting::constant_cost(cost_map["choose_list-cpu-arguments"]),
            ),
            choose_unit: TwoArgumentsCosting::new(
                TwoArgumentsCosting::constant_cost(cost_map["choose_unit-mem-arguments"]),
                TwoArgumentsCosting::constant_cost(cost_map["choose_unit-cpu-arguments"]),
            ),
            cons_byte_string: TwoArgumentsCosting::new(
                TwoArgumentsCosting::added_sizes(
                    cost_map["cons_byte_string-mem-arguments-intercept"],
                    cost_map["cons_byte_string-mem-arguments-slope"],
                ),
                TwoArgumentsCosting::linear_in_y(
                    cost_map["cons_byte_string-cpu-arguments-intercept"],
                    cost_map["cons_byte_string-cpu-arguments-slope"],
                ),
            ),
            constr_data: TwoArgumentsCosting::new(
                TwoArgumentsCosting::constant_cost(cost_map["constr_data-mem-arguments"]),
                TwoArgumentsCosting::constant_cost(cost_map["constr_data-cpu-arguments"]),
            ),
            decode_utf8: OneArgumentCosting::new(
                OneArgumentCosting::linear_cost(
                    cost_map["decode_utf8-mem-arguments-intercept"],
                    cost_map["decode_utf8-mem-arguments-slope"],
                ),
                OneArgumentCosting::linear_cost(
                    cost_map["decode_utf8-cpu-arguments-intercept"],
                    cost_map["decode_utf8-cpu-arguments-slope"],
                ),
            ),
            divide_integer: TwoArgumentsCosting::new(
                TwoArgumentsCosting::subtracted_sizes(
                    cost_map["divide_integer-mem-arguments-intercept"],
                    cost_map["divide_integer-mem-arguments-slope"],
                    cost_map["divide_integer-mem-arguments-minimum"],
                ),
                TwoArgumentsCosting::const_above_diagonal_into_multiplied_sizes(
                    cost_map["divide_integer-cpu-arguments-constant"],
                    cost_map["divide_integer-cpu-arguments-model-arguments-intercept"],
                    cost_map["divide_integer-cpu-arguments-model-arguments-slope"],
                ),
            ),
            encode_utf8: OneArgumentCosting::new(
                OneArgumentCosting::linear_cost(
                    cost_map["encode_utf8-mem-arguments-intercept"],
                    cost_map["encode_utf8-mem-arguments-slope"],
                ),
                OneArgumentCosting::linear_cost(
                    cost_map["encode_utf8-cpu-arguments-intercept"],
                    cost_map["encode_utf8-cpu-arguments-slope"],
                ),
            ),
            equals_byte_string: TwoArgumentsCosting::new(
                TwoArgumentsCosting::constant_cost(cost_map["equals_byte_string-mem-arguments"]),
                TwoArgumentsCosting::linear_on_diagonal(
                    cost_map["equals_byte_string-cpu-arguments-constant"],
                    cost_map["equals_byte_string-cpu-arguments-intercept"],
                    cost_map["equals_byte_string-cpu-arguments-slope"],
                ),
            ),
            equals_data: TwoArgumentsCosting::new(
                TwoArgumentsCosting::constant_cost(cost_map["equals_data-mem-arguments"]),
                TwoArgumentsCosting::min_size(
                    cost_map["equals_data-cpu-arguments-intercept"],
                    cost_map["equals_data-cpu-arguments-slope"],
                ),
            ),
            equals_integer: TwoArgumentsCosting::new(
                TwoArgumentsCosting::constant_cost(cost_map["equals_integer-mem-arguments"]),
                TwoArgumentsCosting::min_size(
                    cost_map["equals_integer-cpu-arguments-intercept"],
                    cost_map["equals_integer-cpu-arguments-slope"],
                ),
            ),
            equals_string: TwoArgumentsCosting::new(
                TwoArgumentsCosting::constant_cost(cost_map["equals_string-mem-arguments"]),
                TwoArgumentsCosting::linear_on_diagonal(
                    cost_map["equals_string-cpu-arguments-constant"],
                    cost_map["equals_string-cpu-arguments-intercept"],
                    cost_map["equals_string-cpu-arguments-slope"],
                ),
            ),
            fst_pair: OneArgumentCosting::new(
                OneArgumentCosting::constant_cost(cost_map["fst_pair-mem-arguments"]),
                OneArgumentCosting::constant_cost(cost_map["fst_pair-cpu-arguments"]),
            ),
            head_list: OneArgumentCosting::new(
                OneArgumentCosting::constant_cost(cost_map["head_list-mem-arguments"]),
                OneArgumentCosting::constant_cost(cost_map["head_list-cpu-arguments"]),
            ),
            i_data: OneArgumentCosting::new(
                OneArgumentCosting::constant_cost(cost_map["i_data-mem-arguments"]),
                OneArgumentCosting::constant_cost(cost_map["i_data-cpu-arguments"]),
            ),
            if_then_else: ThreeArgumentsCosting::new(
                ThreeArgumentsCosting::constant_cost(cost_map["if_then_else-mem-arguments"]),
                ThreeArgumentsCosting::constant_cost(cost_map["if_then_else-cpu-arguments"]),
            ),
            index_byte_string: TwoArgumentsCosting::new(
                TwoArgumentsCosting::constant_cost(cost_map["index_byte_string-mem-arguments"]),
                TwoArgumentsCosting::constant_cost(cost_map["index_byte_string-cpu-arguments"]),
            ),
            length_of_byte_string: OneArgumentCosting::new(
                OneArgumentCosting::constant_cost(cost_map["length_of_byte_string-mem-arguments"]),
                OneArgumentCosting::constant_cost(cost_map["length_of_byte_string-cpu-arguments"]),
            ),
            less_than_byte_string: TwoArgumentsCosting::new(
                TwoArgumentsCosting::constant_cost(cost_map["less_than_byte_string-mem-arguments"]),
                TwoArgumentsCosting::min_size(
                    cost_map["less_than_byte_string-cpu-arguments-intercept"],
                    cost_map["less_than_byte_string-cpu-arguments-slope"],
                ),
            ),
            less_than_equals_byte_string: TwoArgumentsCosting::new(
                TwoArgumentsCosting::constant_cost(cost_map["less_than_equals_byte_string-mem-arguments"]),
                TwoArgumentsCosting::min_size(
                    cost_map["less_than_equals_byte_string-cpu-arguments-intercept"],
                    cost_map["less_than_equals_byte_string-cpu-arguments-slope"],
                ),
            ),
            less_than_equals_integer: TwoArgumentsCosting::new(
                TwoArgumentsCosting::constant_cost(cost_map["less_than_equals_integer-mem-arguments"]),
                TwoArgumentsCosting::min_size(
                    cost_map["less_than_equals_integer-cpu-arguments-intercept"],
                    cost_map["less_than_equals_integer-cpu-arguments-slope"],
                ),
            ),
            less_than_integer: TwoArgumentsCosting::new(
                TwoArgumentsCosting::constant_cost(cost_map["less_than_integer-mem-arguments"]),
                TwoArgumentsCosting::min_size(
                    cost_map["less_than_integer-cpu-arguments-intercept"],
                    cost_map["less_than_integer-cpu-arguments-slope"],
                ),
            ),
            list_data: OneArgumentCosting::new(
                OneArgumentCosting::constant_cost(cost_map["list_data-mem-arguments"]),
                OneArgumentCosting::constant_cost(cost_map["list_data-cpu-arguments"]),
            ),
            map_data: OneArgumentCosting::new(
                OneArgumentCosting::constant_cost(cost_map["map_data-mem-arguments"]),
                OneArgumentCosting::constant_cost(cost_map["map_data-cpu-arguments"]),
            ),
            mk_cons: TwoArgumentsCosting::new(
                TwoArgumentsCosting::constant_cost(cost_map["mk_cons-mem-arguments"]),
                TwoArgumentsCosting::constant_cost(cost_map["mk_cons-cpu-arguments"]),
            ),
            mk_nil_data: OneArgumentCosting::new(
                OneArgumentCosting::constant_cost(cost_map["mk_nil_data-mem-arguments"]),
                OneArgumentCosting::constant_cost(cost_map["mk_nil_data-cpu-arguments"]),
            ),
            mk_nil_pair_data: OneArgumentCosting::new(
                OneArgumentCosting::constant_cost(cost_map["mk_nil_pair_data-mem-arguments"]),
                OneArgumentCosting::constant_cost(cost_map["mk_nil_pair_data-cpu-arguments"]),
            ),
            mk_pair_data: TwoArgumentsCosting::new(
                TwoArgumentsCosting::constant_cost(cost_map["mk_pair_data-mem-arguments"]),
                TwoArgumentsCosting::constant_cost(cost_map["mk_pair_data-cpu-arguments"]),
            ),
            mod_integer: TwoArgumentsCosting::new(
                TwoArgumentsCosting::subtracted_sizes(
                    cost_map["mod_integer-mem-arguments-intercept"],
                    cost_map["mod_integer-mem-arguments-slope"],
                    cost_map["mod_integer-mem-arguments-minimum"],
                ),
                TwoArgumentsCosting::const_above_diagonal_into_multiplied_sizes(
                    cost_map["mod_integer-cpu-arguments-constant"],
                    cost_map["mod_integer-cpu-arguments-model-arguments-intercept"],
                    cost_map["mod_integer-cpu-arguments-model-arguments-slope"],
                ),
            ),
            multiply_integer: TwoArgumentsCosting::new(
                TwoArgumentsCosting::added_sizes(
                    cost_map["multiply_integer-mem-arguments-intercept"],
                    cost_map["multiply_integer-mem-arguments-slope"],
                ),
                TwoArgumentsCosting::multiplied_sizes(
                    cost_map["multiply_integer-cpu-arguments-intercept"],
                    cost_map["multiply_integer-cpu-arguments-slope"],
                ),
            ),
            null_list: OneArgumentCosting::new(
                OneArgumentCosting::constant_cost(cost_map["null_list-mem-arguments"]),
                OneArgumentCosting::constant_cost(cost_map["null_list-cpu-arguments"]),
            ),
            quotient_integer: TwoArgumentsCosting::new(
                TwoArgumentsCosting::subtracted_sizes(
                    cost_map["quotient_integer-mem-arguments-intercept"],
                    cost_map["quotient_integer-mem-arguments-slope"],
                    cost_map["quotient_integer-mem-arguments-minimum"],
                ),
                TwoArgumentsCosting::const_above_diagonal_into_multiplied_sizes(
                    cost_map["quotient_integer-cpu-arguments-constant"],
                    cost_map["quotient_integer-cpu-arguments-model-arguments-intercept"],
                    cost_map["quotient_integer-cpu-arguments-model-arguments-slope"],
                ),
            ),
            remainder_integer: TwoArgumentsCosting::new(
                TwoArgumentsCosting::subtracted_sizes(
                    cost_map["remainder_integer-mem-arguments-intercept"],
                    cost_map["remainder_integer-mem-arguments-slope"],
                    cost_map["remainder_integer-mem-arguments-minimum"],
                ),
                TwoArgumentsCosting::const_above_diagonal_into_multiplied_sizes(
                    cost_map["remainder_integer-cpu-arguments-constant"],
                    cost_map["remainder_integer-cpu-arguments-model-arguments-intercept"],
                    cost_map["remainder_integer-cpu-arguments-model-arguments-slope"],
                ),
            ),
            sha2_256: OneArgumentCosting::new(
                OneArgumentCosting::constant_cost(cost_map["sha2_256-mem-arguments"]),
                OneArgumentCosting::linear_cost(
                    cost_map["sha2_256-cpu-arguments-intercept"],
                    cost_map["sha2_256-cpu-arguments-slope"],
                ),
            ),
            sha3_256: OneArgumentCosting::new(
                OneArgumentCosting::constant_cost(cost_map["sha3_256-mem-arguments"]),
                OneArgumentCosting::linear_cost(
                    cost_map["sha3_256-cpu-arguments-intercept"],
                    cost_map["sha3_256-cpu-arguments-slope"],
                ),
            ),
            slice_byte_string: ThreeArgumentsCosting::new(
                ThreeArgumentsCosting::linear_in_z(
                    cost_map["slice_byte_string-mem-arguments-intercept"],
                    cost_map["slice_byte_string-mem-arguments-slope"],
                ),
                ThreeArgumentsCosting::linear_in_z(
                    cost_map["slice_byte_string-cpu-arguments-intercept"],
                    cost_map["slice_byte_string-cpu-arguments-slope"],
                ),
            ),
            snd_pair: OneArgumentCosting::new(
                OneArgumentCosting::constant_cost(cost_map["snd_pair-mem-arguments"]),
                OneArgumentCosting::constant_cost(cost_map["snd_pair-cpu-arguments"]),
            ),
            subtract_integer: TwoArgumentsCosting::new(
                TwoArgumentsCosting::max_size(
                    cost_map["subtract_integer-mem-arguments-intercept"],
                    cost_map["subtract_integer-mem-arguments-slope"],
                ),
                TwoArgumentsCosting::max_size(
                    cost_map["subtract_integer-cpu-arguments-intercept"],
                    cost_map["subtract_integer-cpu-arguments-slope"],
                ),
            ),
            tail_list: OneArgumentCosting::new(
                OneArgumentCosting::constant_cost(cost_map["tail_list-mem-arguments"]),
                OneArgumentCosting::constant_cost(cost_map["tail_list-cpu-arguments"]),
            ),
            trace: TwoArgumentsCosting::new(
                TwoArgumentsCosting::constant_cost(cost_map["trace-mem-arguments"]),
                TwoArgumentsCosting::constant_cost(cost_map["trace-cpu-arguments"]),
            ),
            un_b_data: OneArgumentCosting::new(
                OneArgumentCosting::constant_cost(cost_map["un_b_data-mem-arguments"]),
                OneArgumentCosting::constant_cost(cost_map["un_b_data-cpu-arguments"]),
            ),
            un_constr_data: OneArgumentCosting::new(
                OneArgumentCosting::constant_cost(cost_map["un_constr_data-mem-arguments"]),
                OneArgumentCosting::constant_cost(cost_map["un_constr_data-cpu-arguments"]),
            ),
            un_i_data: OneArgumentCosting::new(
                OneArgumentCosting::constant_cost(cost_map["un_i_data-mem-arguments"]),
                OneArgumentCosting::constant_cost(cost_map["un_i_data-cpu-arguments"]),
            ),
            un_list_data: OneArgumentCosting::new(
                OneArgumentCosting::constant_cost(cost_map["un_list_data-mem-arguments"]),
                OneArgumentCosting::constant_cost(cost_map["un_list_data-cpu-arguments"]),
            ),
            un_map_data: OneArgumentCosting::new(
                OneArgumentCosting::constant_cost(cost_map["un_map_data-mem-arguments"]),
                OneArgumentCosting::constant_cost(cost_map["un_map_data-cpu-arguments"]),
            ),
            verify_ed25519_signature: ThreeArgumentsCosting::new(
                ThreeArgumentsCosting::constant_cost(cost_map["verify_ed25519_signature-mem-arguments"]),
                ThreeArgumentsCosting::linear_in_y(
                    cost_map["verify_ed25519_signature-cpu-arguments-intercept"],
                    cost_map["verify_ed25519_signature-cpu-arguments-slope"],
                ),
            ),
            integer_to_byte_string: ThreeArgumentsCosting::new(
                ThreeArgumentsCosting::literal_in_y_or_linear_in_z(
                    cost_map["integerToByteString-mem-arguments-intercept"],
                    cost_map["integerToByteString-mem-arguments-slope"],
                ),
                ThreeArgumentsCosting::quadratic_in_z(
                    cost_map["integerToByteString-cpu-arguments-c0"],
                    cost_map["integerToByteString-cpu-arguments-c1"],
                    cost_map["integerToByteString-cpu-arguments-c2"],
                ),
            ),

            byte_string_to_integer: TwoArgumentsCosting::new(
                TwoArgumentsCosting::linear_in_y(
                    cost_map["byteStringToInteger-mem-arguments-intercept"],
                    cost_map["byteStringToInteger-mem-arguments-slope"],
                ),
                TwoArgumentsCosting::quadratic_in_y(
                    cost_map["byteStringToInteger-cpu-arguments-c0"],
                    cost_map["byteStringToInteger-cpu-arguments-c1"],
                    cost_map["byteStringToInteger-cpu-arguments-c2"],
                ),
            ),

            ripemd_160: OneArgumentCosting::new(
                OneArgumentCosting::constant_cost(cost_map["ripemd_160-memory-arguments"]),
                OneArgumentCosting::linear_cost(
                    cost_map["ripemd_160-cpu-arguments-intercept"],
                    cost_map["ripemd_160-cpu-arguments-slope"],
                ),
            ),

            exp_mod_integer: ThreeArgumentsCosting::new(
                ThreeArgumentsCosting::linear_in_z(
                    cost_map["exp_mod_integer-mem-arguments-intercept"],
                    cost_map["exp_mod_integer-mem-arguments-slope"],
                ),
                ThreeArgumentsCosting::exp_mod_cost(
                    cost_map["exp_mod_integer-cpu-arguments-coefficient00"],
                    cost_map["exp_mod_integer-cpu-arguments-coefficient11"],
                    cost_map["exp_mod_integer-cpu-arguments-coefficient12"],
                ),
            ),

            drop_list: TwoArgumentsCosting::new(
                TwoArgumentsCosting::constant_cost(cost_map["drop_list-mem-arguments"]),
                TwoArgumentsCosting::linear_in_x(
                    cost_map["drop_list-cpu-arguments-intercept"],
                    cost_map["drop_list-cpu-arguments-slope"],
                ),
            ),
            length_of_array: OneArgumentCosting::new(
                OneArgumentCosting::constant_cost(cost_map["length_of_array-mem-arguments"]),
                OneArgumentCosting::constant_cost(cost_map["length_of_array-cpu-arguments"]),
            ),
            list_to_array: TwoArgumentsCosting::new(
                TwoArgumentsCosting::linear_in_x(
                    cost_map["list_to_array-mem-arguments-intercept"],
                    cost_map["list_to_array-mem-arguments-slope"],
                ),
                TwoArgumentsCosting::linear_in_x(
                    cost_map["list_to_array-cpu-arguments-intercept"],
                    cost_map["list_to_array-cpu-arguments-slope"],
                ),
            ),
            index_array: TwoArgumentsCosting::new(
                TwoArgumentsCosting::constant_cost(cost_map["index_array-mem-arguments"]),
                TwoArgumentsCosting::constant_cost(cost_map["index_array-cpu-arguments"]),
            ),
        }
    }

    fn get_cost(&self, builtin: DefaultFunction, args: &[i64]) -> Option<ExBudget> {
        match builtin {
            DefaultFunction::AddInteger => Some(ExBudget::new(
                self.add_integer.mem.cost([args[0], args[1]]),
                self.add_integer.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::SubtractInteger => Some(ExBudget::new(
                self.subtract_integer.mem.cost([args[0], args[1]]),
                self.subtract_integer.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::MultiplyInteger => Some(ExBudget::new(
                self.multiply_integer.mem.cost([args[0], args[1]]),
                self.multiply_integer.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::DivideInteger => Some(ExBudget::new(
                self.divide_integer.mem.cost([args[0], args[1]]),
                self.divide_integer.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::QuotientInteger => Some(ExBudget::new(
                self.quotient_integer.mem.cost([args[0], args[1]]),
                self.quotient_integer.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::RemainderInteger => Some(ExBudget::new(
                self.remainder_integer.mem.cost([args[0], args[1]]),
                self.remainder_integer.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::ModInteger => Some(ExBudget::new(
                self.mod_integer.mem.cost([args[0], args[1]]),
                self.mod_integer.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::EqualsInteger => Some(ExBudget::new(
                self.equals_integer.mem.cost([args[0], args[1]]),
                self.equals_integer.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::LessThanInteger => Some(ExBudget::new(
                self.less_than_integer.mem.cost([args[0], args[1]]),
                self.less_than_integer.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::LessThanEqualsInteger => Some(ExBudget::new(
                self.less_than_equals_integer.mem.cost([args[0], args[1]]),
                self.less_than_equals_integer.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::AppendByteString => Some(ExBudget::new(
                self.append_byte_string.mem.cost([args[0], args[1]]),
                self.append_byte_string.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::ConsByteString => Some(ExBudget::new(
                self.cons_byte_string.mem.cost([args[0], args[1]]),
                self.cons_byte_string.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::SliceByteString => Some(ExBudget::new(
                self.slice_byte_string.mem.cost([args[0], args[1], args[2]]),
                self.slice_byte_string.cpu.cost([args[0], args[1], args[2]]),
            )),
            DefaultFunction::LengthOfByteString => Some(ExBudget::new(
                self.length_of_byte_string.mem.cost([args[0]]),
                self.length_of_byte_string.cpu.cost([args[0]]),
            )),
            DefaultFunction::IndexByteString => Some(ExBudget::new(
                self.index_byte_string.mem.cost([args[0], args[1]]),
                self.index_byte_string.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::EqualsByteString => Some(ExBudget::new(
                self.equals_byte_string.mem.cost([args[0], args[1]]),
                self.equals_byte_string.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::LessThanByteString => Some(ExBudget::new(
                self.less_than_byte_string.mem.cost([args[0], args[1]]),
                self.less_than_byte_string.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::LessThanEqualsByteString => Some(ExBudget::new(
                self.less_than_equals_byte_string.mem.cost([args[0], args[1]]),
                self.less_than_equals_byte_string.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::Sha2_256 => {
                Some(ExBudget::new(self.sha2_256.mem.cost([args[0]]), self.sha2_256.cpu.cost([args[0]])))
            }
            DefaultFunction::Sha3_256 => {
                Some(ExBudget::new(self.sha3_256.mem.cost([args[0]]), self.sha3_256.cpu.cost([args[0]])))
            }
            DefaultFunction::Blake2b_256 => {
                Some(ExBudget::new(self.blake2b_256.mem.cost([args[0]]), self.blake2b_256.cpu.cost([args[0]])))
            }
            DefaultFunction::VerifyEd25519Signature => Some(ExBudget::new(
                self.verify_ed25519_signature.mem.cost([args[0], args[1], args[2]]),
                self.verify_ed25519_signature.cpu.cost([args[0], args[1], args[2]]),
            )),
            DefaultFunction::AppendString => Some(ExBudget::new(
                self.append_string.mem.cost([args[0], args[1]]),
                self.append_string.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::EqualsString => Some(ExBudget::new(
                self.equals_string.mem.cost([args[0], args[1]]),
                self.equals_string.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::EncodeUtf8 => {
                Some(ExBudget::new(self.encode_utf8.mem.cost([args[0]]), self.encode_utf8.cpu.cost([args[0]])))
            }
            DefaultFunction::DecodeUtf8 => {
                Some(ExBudget::new(self.decode_utf8.mem.cost([args[0]]), self.decode_utf8.cpu.cost([args[0]])))
            }
            DefaultFunction::IfThenElse => Some(ExBudget::new(
                self.if_then_else.mem.cost([args[0], args[1], args[2]]),
                self.if_then_else.cpu.cost([args[0], args[1], args[2]]),
            )),
            DefaultFunction::ChooseUnit => Some(ExBudget::new(
                self.choose_unit.mem.cost([args[0], args[1]]),
                self.choose_unit.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::Trace => {
                Some(ExBudget::new(self.trace.mem.cost([args[0], args[1]]), self.trace.cpu.cost([args[0], args[1]])))
            }
            DefaultFunction::FstPair => {
                Some(ExBudget::new(self.fst_pair.mem.cost([args[0]]), self.fst_pair.cpu.cost([args[0]])))
            }
            DefaultFunction::SndPair => {
                Some(ExBudget::new(self.snd_pair.mem.cost([args[0]]), self.snd_pair.cpu.cost([args[0]])))
            }
            DefaultFunction::ChooseList => Some(ExBudget::new(
                self.choose_list.mem.cost([args[0], args[1], args[2]]),
                self.choose_list.cpu.cost([args[0], args[1], args[2]]),
            )),
            DefaultFunction::MkCons => Some(ExBudget::new(
                self.mk_cons.mem.cost([args[0], args[1]]),
                self.mk_cons.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::HeadList => {
                Some(ExBudget::new(self.head_list.mem.cost([args[0]]), self.head_list.cpu.cost([args[0]])))
            }
            DefaultFunction::TailList => {
                Some(ExBudget::new(self.tail_list.mem.cost([args[0]]), self.tail_list.cpu.cost([args[0]])))
            }
            DefaultFunction::NullList => {
                Some(ExBudget::new(self.null_list.mem.cost([args[0]]), self.null_list.cpu.cost([args[0]])))
            }
            DefaultFunction::ChooseData => Some(ExBudget::new(
                self.choose_data.mem.cost([args[0], args[1], args[2], args[3], args[4], args[5]]),
                self.choose_data.cpu.cost([args[0], args[1], args[2], args[3], args[4], args[5]]),
            )),
            DefaultFunction::ConstrData => Some(ExBudget::new(
                self.constr_data.mem.cost([args[0], args[1]]),
                self.constr_data.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::MapData => {
                Some(ExBudget::new(self.map_data.mem.cost([args[0]]), self.map_data.cpu.cost([args[0]])))
            }
            DefaultFunction::ListData => {
                Some(ExBudget::new(self.list_data.mem.cost([args[0]]), self.list_data.cpu.cost([args[0]])))
            }
            DefaultFunction::IData => {
                Some(ExBudget::new(self.i_data.mem.cost([args[0]]), self.i_data.cpu.cost([args[0]])))
            }
            DefaultFunction::BData => {
                Some(ExBudget::new(self.b_data.mem.cost([args[0]]), self.b_data.cpu.cost([args[0]])))
            }
            DefaultFunction::UnConstrData => {
                Some(ExBudget::new(self.un_constr_data.mem.cost([args[0]]), self.un_constr_data.cpu.cost([args[0]])))
            }
            DefaultFunction::UnMapData => {
                Some(ExBudget::new(self.un_map_data.mem.cost([args[0]]), self.un_map_data.cpu.cost([args[0]])))
            }
            DefaultFunction::UnListData => {
                Some(ExBudget::new(self.un_list_data.mem.cost([args[0]]), self.un_list_data.cpu.cost([args[0]])))
            }
            DefaultFunction::UnIData => {
                Some(ExBudget::new(self.un_i_data.mem.cost([args[0]]), self.un_i_data.cpu.cost([args[0]])))
            }
            DefaultFunction::UnBData => {
                Some(ExBudget::new(self.un_b_data.mem.cost([args[0]]), self.un_b_data.cpu.cost([args[0]])))
            }
            DefaultFunction::EqualsData => Some(ExBudget::new(
                self.equals_data.mem.cost([args[0], args[1]]),
                self.equals_data.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::MkPairData => Some(ExBudget::new(
                self.mk_pair_data.mem.cost([args[0], args[1]]),
                self.mk_pair_data.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::MkNilData => {
                Some(ExBudget::new(self.mk_nil_data.mem.cost([args[0]]), self.mk_nil_data.cpu.cost([args[0]])))
            }
            DefaultFunction::MkNilPairData => Some(ExBudget::new(
                self.mk_nil_pair_data.mem.cost([args[0]]),
                self.mk_nil_pair_data.cpu.cost([args[0]]),
            )),
            DefaultFunction::Ripemd_160 => {
                Some(ExBudget::new(self.ripemd_160.mem.cost([args[0]]), self.ripemd_160.cpu.cost([args[0]])))
            }
            DefaultFunction::ExpModInteger => Some(ExBudget::new(
                self.exp_mod_integer.mem.cost([args[0], args[1], args[2]]),
                self.exp_mod_integer.cpu.cost([args[0], args[1], args[2]]),
            )),
            DefaultFunction::DropList => Some(ExBudget::new(
                self.drop_list.mem.cost([args[0], args[1]]),
                self.drop_list.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::LengthOfArray => {
                Some(ExBudget::new(self.length_of_array.mem.cost([args[0]]), self.length_of_array.cpu.cost([args[0]])))
            }
            DefaultFunction::ListToArray => Some(ExBudget::new(
                self.list_to_array.mem.cost([args[0], args[1]]),
                self.list_to_array.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::IndexArray => Some(ExBudget::new(
                self.index_array.mem.cost([args[0], args[1]]),
                self.index_array.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::IntegerToByteString => Some(ExBudget::new(
                self.integer_to_byte_string.mem.cost([args[0], args[1], args[2]]),
                self.integer_to_byte_string.cpu.cost([args[0], args[1], args[2]]),
            )),
            DefaultFunction::ByteStringToInteger => Some(ExBudget::new(
                self.byte_string_to_integer.mem.cost([args[0], args[1]]),
                self.byte_string_to_integer.cpu.cost([args[0], args[1]]),
            )),
            _ => None,
        }
    }
}
