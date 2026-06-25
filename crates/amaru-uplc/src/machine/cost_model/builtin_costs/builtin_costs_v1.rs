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
    blake2b_224: OneArgumentCosting,
    blake2b_256: OneArgumentCosting,
    keccak_256: OneArgumentCosting,
    verify_ed25519_signature: ThreeArgumentsCosting,
    verify_ecdsa_secp256k1_signature: ThreeArgumentsCosting,
    verify_schnorr_secp256k1_signature: ThreeArgumentsCosting,
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
    serialise_data: OneArgumentCosting,
    // BLST
    bls12_381_g1_add: TwoArgumentsCosting,
    bls12_381_g1_neg: OneArgumentCosting,
    bls12_381_g1_scalar_mul: TwoArgumentsCosting,
    bls12_381_g1_equal: TwoArgumentsCosting,
    bls12_381_g1_compress: OneArgumentCosting,
    bls12_381_g1_uncompress: OneArgumentCosting,
    bls12_381_g1_hash_to_group: TwoArgumentsCosting,
    bls12_381_g2_add: TwoArgumentsCosting,
    bls12_381_g2_neg: OneArgumentCosting,
    bls12_381_g2_scalar_mul: TwoArgumentsCosting,
    bls12_381_g2_equal: TwoArgumentsCosting,
    bls12_381_g2_compress: OneArgumentCosting,
    bls12_381_g2_uncompress: OneArgumentCosting,
    bls12_381_g2_hash_to_group: TwoArgumentsCosting,
    bls12_381_miller_loop: TwoArgumentsCosting,
    bls12_381_mul_ml_result: TwoArgumentsCosting,
    bls12_381_final_verify: TwoArgumentsCosting,
    // bitwise
    integer_to_byte_string: ThreeArgumentsCosting,
    byte_string_to_integer: TwoArgumentsCosting,
    and_byte_string: ThreeArgumentsCosting,
    or_byte_string: ThreeArgumentsCosting,
    xor_byte_string: ThreeArgumentsCosting,
    complement_byte_string: OneArgumentCosting,
    read_bit: TwoArgumentsCosting,
    write_bits: ThreeArgumentsCosting,
    replicate_byte: TwoArgumentsCosting,
    shift_byte_string: TwoArgumentsCosting,
    rotate_byte_string: TwoArgumentsCosting,
    count_set_bits: OneArgumentCosting,
    find_first_set_bit: OneArgumentCosting,
    ripemd_160: OneArgumentCosting,

    exp_mod_integer: ThreeArgumentsCosting,
    drop_list: TwoArgumentsCosting,
    length_of_array: OneArgumentCosting,
    list_to_array: TwoArgumentsCosting,
    index_array: TwoArgumentsCosting,

    bls12_381_g1_multi_scalar_mul: TwoArgumentsCosting,
    bls12_381_g2_multi_scalar_mul: TwoArgumentsCosting,
    // Value operations
    insert_coin: OneArgumentCosting,
    lookup_coin: ThreeArgumentsCosting,
    union_value: TwoArgumentsCosting,
    value_contains: TwoArgumentsCosting,
    value_data: OneArgumentCosting,
    un_value_data: OneArgumentCosting,
    scale_value: TwoArgumentsCosting,
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
                TwoArgumentsCosting::linear_on_diagonal(30623, 28755, 75),
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
            verify_ecdsa_secp256k1_signature: ThreeArgumentsCosting::new(
                ThreeArgumentsCosting::constant_cost(10),
                ThreeArgumentsCosting::constant_cost(43053543),
            ),
            verify_schnorr_secp256k1_signature: ThreeArgumentsCosting::new(
                ThreeArgumentsCosting::constant_cost(10),
                ThreeArgumentsCosting::linear_in_y(43574283, 26308),
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

            serialise_data: OneArgumentCosting::new(
                OneArgumentCosting::linear_cost(0, 2),
                OneArgumentCosting::linear_cost(955506, 213312),
            ),
            blake2b_224: OneArgumentCosting::new(
                OneArgumentCosting::constant_cost(4),
                OneArgumentCosting::linear_cost(207616, 8310),
            ),
            keccak_256: OneArgumentCosting::new(
                OneArgumentCosting::constant_cost(4),
                OneArgumentCosting::linear_cost(2261318, 64571),
            ),
            bls12_381_g1_add: TwoArgumentsCosting::new(
                TwoArgumentsCosting::constant_cost(18),
                TwoArgumentsCosting::constant_cost(962335),
            ),
            bls12_381_g1_neg: OneArgumentCosting::new(
                OneArgumentCosting::constant_cost(18),
                OneArgumentCosting::constant_cost(267929),
            ),
            bls12_381_g1_scalar_mul: TwoArgumentsCosting::new(
                TwoArgumentsCosting::constant_cost(18),
                TwoArgumentsCosting::linear_in_x(76433006, 8868),
            ),
            bls12_381_g1_equal: TwoArgumentsCosting::new(
                TwoArgumentsCosting::constant_cost(1),
                TwoArgumentsCosting::constant_cost(442008),
            ),
            bls12_381_g1_compress: OneArgumentCosting::new(
                OneArgumentCosting::constant_cost(6),
                OneArgumentCosting::constant_cost(2780678),
            ),
            bls12_381_g1_uncompress: OneArgumentCosting::new(
                OneArgumentCosting::constant_cost(18),
                OneArgumentCosting::constant_cost(52948122),
            ),
            bls12_381_g1_hash_to_group: TwoArgumentsCosting::new(
                TwoArgumentsCosting::constant_cost(18),
                TwoArgumentsCosting::linear_in_x(52538055, 3756),
            ),
            bls12_381_g2_add: TwoArgumentsCosting::new(
                TwoArgumentsCosting::constant_cost(36),
                TwoArgumentsCosting::constant_cost(1995836),
            ),
            bls12_381_g2_neg: OneArgumentCosting::new(
                OneArgumentCosting::constant_cost(36),
                OneArgumentCosting::constant_cost(284546),
            ),
            bls12_381_g2_scalar_mul: TwoArgumentsCosting::new(
                TwoArgumentsCosting::constant_cost(36),
                TwoArgumentsCosting::linear_in_x(158_221_314, 26_549),
            ),
            bls12_381_g2_equal: TwoArgumentsCosting::new(
                TwoArgumentsCosting::constant_cost(1),
                TwoArgumentsCosting::constant_cost(901_022),
            ),
            bls12_381_g2_compress: OneArgumentCosting::new(
                OneArgumentCosting::constant_cost(12),
                OneArgumentCosting::constant_cost(3_227_919),
            ),
            bls12_381_g2_uncompress: OneArgumentCosting::new(
                OneArgumentCosting::constant_cost(36),
                OneArgumentCosting::constant_cost(74_698_472),
            ),
            bls12_381_g2_hash_to_group: TwoArgumentsCosting::new(
                TwoArgumentsCosting::constant_cost(36),
                TwoArgumentsCosting::linear_in_x(166_917_843, 4_307),
            ),
            bls12_381_miller_loop: TwoArgumentsCosting::new(
                TwoArgumentsCosting::constant_cost(72),
                TwoArgumentsCosting::constant_cost(254006273),
            ),
            bls12_381_mul_ml_result: TwoArgumentsCosting::new(
                TwoArgumentsCosting::constant_cost(72),
                TwoArgumentsCosting::constant_cost(2174038),
            ),
            bls12_381_final_verify: TwoArgumentsCosting::new(
                TwoArgumentsCosting::constant_cost(1),
                TwoArgumentsCosting::constant_cost(333849714),
            ),
            integer_to_byte_string: ThreeArgumentsCosting::new(
                ThreeArgumentsCosting::literal_in_y_or_linear_in_z(0, 1),
                ThreeArgumentsCosting::quadratic_in_z(1293828, 28716, 63),
            ),
            byte_string_to_integer: TwoArgumentsCosting::new(
                TwoArgumentsCosting::linear_in_y(0, 1),
                TwoArgumentsCosting::quadratic_in_y(1006041, 43623, 251),
            ),
            and_byte_string: ThreeArgumentsCosting::new(
                ThreeArgumentsCosting::linear_in_max_y_z(0, 1),
                ThreeArgumentsCosting::linear_in_y_and_z(100181, 726, 719),
            ),
            or_byte_string: ThreeArgumentsCosting::new(
                ThreeArgumentsCosting::linear_in_max_y_z(0, 1),
                ThreeArgumentsCosting::linear_in_y_and_z(100181, 726, 719),
            ),
            xor_byte_string: ThreeArgumentsCosting::new(
                ThreeArgumentsCosting::linear_in_max_y_z(0, 1),
                ThreeArgumentsCosting::linear_in_y_and_z(100181, 726, 719),
            ),
            complement_byte_string: OneArgumentCosting::new(
                OneArgumentCosting::linear_cost(0, 1),
                OneArgumentCosting::linear_cost(107878, 680),
            ),
            read_bit: TwoArgumentsCosting::new(
                TwoArgumentsCosting::constant_cost(1),
                TwoArgumentsCosting::constant_cost(95336),
            ),
            write_bits: ThreeArgumentsCosting::new(
                ThreeArgumentsCosting::linear_in_x(0, 1),
                ThreeArgumentsCosting::linear_in_y(281145, 18848),
            ),
            replicate_byte: TwoArgumentsCosting::new(
                TwoArgumentsCosting::linear_in_x(1, 1),
                TwoArgumentsCosting::linear_in_x(180194, 159),
            ),
            shift_byte_string: TwoArgumentsCosting::new(
                TwoArgumentsCosting::linear_in_x(0, 1),
                TwoArgumentsCosting::linear_in_x(158519, 8942),
            ),
            rotate_byte_string: TwoArgumentsCosting::new(
                TwoArgumentsCosting::linear_in_x(0, 1),
                TwoArgumentsCosting::linear_in_x(159378, 8813),
            ),
            count_set_bits: OneArgumentCosting::new(
                OneArgumentCosting::constant_cost(1),
                OneArgumentCosting::linear_cost(107490, 3298),
            ),
            find_first_set_bit: OneArgumentCosting::new(
                OneArgumentCosting::constant_cost(1),
                OneArgumentCosting::linear_cost(106057, 655),
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
                OneArgumentCosting::constant_cost(231883),
            ),
            list_to_array: TwoArgumentsCosting::new(
                TwoArgumentsCosting::linear_in_x(7, 1),
                TwoArgumentsCosting::linear_in_x(1000, 24838),
            ),
            index_array: TwoArgumentsCosting::new(
                TwoArgumentsCosting::constant_cost(32),
                TwoArgumentsCosting::constant_cost(232010),
            ),
            bls12_381_g1_multi_scalar_mul: TwoArgumentsCosting::new(
                TwoArgumentsCosting::constant_cost(18),
                TwoArgumentsCosting::linear_in_x(321837444, 25087669),
            ),
            bls12_381_g2_multi_scalar_mul: TwoArgumentsCosting::new(
                TwoArgumentsCosting::constant_cost(36),
                TwoArgumentsCosting::linear_in_x(617887431, 67302824),
            ),
            insert_coin: OneArgumentCosting::new(
                OneArgumentCosting::linear_cost(45, 21),
                OneArgumentCosting::linear_cost(356924, 18413),
            ),
            lookup_coin: ThreeArgumentsCosting::new(
                ThreeArgumentsCosting::constant_cost(1),
                ThreeArgumentsCosting::linear_in_z(219951, 9444),
            ),
            union_value: TwoArgumentsCosting::new(
                TwoArgumentsCosting::added_sizes(24, 21),
                TwoArgumentsCosting::with_interaction(1000, 172116, 183150, 6),
            ),
            value_contains: TwoArgumentsCosting::new(
                TwoArgumentsCosting::constant_cost(1),
                TwoArgumentsCosting::const_above_diagonal_into_linear_in_x_and_y(213283, 618401, 1998, 28258),
            ),
            value_data: OneArgumentCosting::new(
                OneArgumentCosting::linear_cost(2, 22),
                OneArgumentCosting::linear_cost(1000, 38159),
            ),
            un_value_data: OneArgumentCosting::new(
                OneArgumentCosting::linear_cost(1, 11),
                OneArgumentCosting::quadratic_cost(1000, 95933, 1),
            ),
            scale_value: TwoArgumentsCosting::new(
                TwoArgumentsCosting::linear_in_y(12, 21),
                TwoArgumentsCosting::linear_in_y(1000, 277577),
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
            serialise_data: OneArgumentCosting::new(
                OneArgumentCosting::linear_cost(
                    cost_map["serialise_data-mem-arguments-intercept"],
                    cost_map["serialise_data-mem-arguments-slope"],
                ),
                OneArgumentCosting::linear_cost(
                    cost_map["serialise_data-cpu-arguments-intercept"],
                    cost_map["serialise_data-cpu-arguments-slope"],
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
            verify_ecdsa_secp256k1_signature: ThreeArgumentsCosting::new(
                ThreeArgumentsCosting::constant_cost(cost_map["verify_ecdsa_secp256k1_signature-mem-arguments"]),
                ThreeArgumentsCosting::constant_cost(cost_map["verify_ecdsa_secp256k1_signature-cpu-arguments"]),
            ),
            verify_ed25519_signature: ThreeArgumentsCosting::new(
                ThreeArgumentsCosting::constant_cost(cost_map["verify_ed25519_signature-mem-arguments"]),
                ThreeArgumentsCosting::linear_in_y(
                    cost_map["verify_ed25519_signature-cpu-arguments-intercept"],
                    cost_map["verify_ed25519_signature-cpu-arguments-slope"],
                ),
            ),
            verify_schnorr_secp256k1_signature: ThreeArgumentsCosting::new(
                ThreeArgumentsCosting::constant_cost(cost_map["verify_schnorr_secp256k1_signature-mem-arguments"]),
                ThreeArgumentsCosting::linear_in_y(
                    cost_map["verify_schnorr_secp256k1_signature-cpu-arguments-intercept"],
                    cost_map["verify_schnorr_secp256k1_signature-cpu-arguments-slope"],
                ),
            ),

            bls12_381_g1_add: TwoArgumentsCosting::new(
                TwoArgumentsCosting::constant_cost(cost_map["bls12_381_G1_add-mem-arguments"]),
                TwoArgumentsCosting::constant_cost(cost_map["bls12_381_G1_add-cpu-arguments"]),
            ),

            bls12_381_g1_compress: OneArgumentCosting::new(
                OneArgumentCosting::constant_cost(cost_map["bls12_381_G1_compress-mem-arguments"]),
                OneArgumentCosting::constant_cost(cost_map["bls12_381_G1_compress-cpu-arguments"]),
            ),

            bls12_381_g1_equal: TwoArgumentsCosting::new(
                TwoArgumentsCosting::constant_cost(cost_map["bls12_381_G1_equal-mem-arguments"]),
                TwoArgumentsCosting::constant_cost(cost_map["bls12_381_G1_equal-cpu-arguments"]),
            ),

            bls12_381_g1_hash_to_group: TwoArgumentsCosting::new(
                TwoArgumentsCosting::constant_cost(cost_map["bls12_381_G1_hashToGroup-mem-arguments"]),
                TwoArgumentsCosting::linear_in_x(
                    cost_map["bls12_381_G1_hashToGroup-cpu-arguments-intercept"],
                    cost_map["bls12_381_G1_hashToGroup-cpu-arguments-slope"],
                ),
            ),

            bls12_381_g1_neg: OneArgumentCosting::new(
                OneArgumentCosting::constant_cost(cost_map["bls12_381_G1_neg-mem-arguments"]),
                OneArgumentCosting::constant_cost(cost_map["bls12_381_G1_neg-cpu-arguments"]),
            ),

            bls12_381_g1_scalar_mul: TwoArgumentsCosting::new(
                TwoArgumentsCosting::constant_cost(cost_map["bls12_381_G1_scalarMul-mem-arguments"]),
                TwoArgumentsCosting::linear_in_x(
                    cost_map["bls12_381_G1_scalarMul-cpu-arguments-intercept"],
                    cost_map["bls12_381_G1_scalarMul-cpu-arguments-slope"],
                ),
            ),

            bls12_381_g1_uncompress: OneArgumentCosting::new(
                OneArgumentCosting::constant_cost(cost_map["bls12_381_G1_uncompress-mem-arguments"]),
                OneArgumentCosting::constant_cost(cost_map["bls12_381_G1_uncompress-cpu-arguments"]),
            ),

            bls12_381_g2_add: TwoArgumentsCosting::new(
                TwoArgumentsCosting::constant_cost(cost_map["bls12_381_G2_add-mem-arguments"]),
                TwoArgumentsCosting::constant_cost(cost_map["bls12_381_G2_add-cpu-arguments"]),
            ),

            bls12_381_g2_compress: OneArgumentCosting::new(
                OneArgumentCosting::constant_cost(cost_map["bls12_381_G2_compress-mem-arguments"]),
                OneArgumentCosting::constant_cost(cost_map["bls12_381_G2_compress-cpu-arguments"]),
            ),

            bls12_381_g2_equal: TwoArgumentsCosting::new(
                TwoArgumentsCosting::constant_cost(cost_map["bls12_381_G2_equal-mem-arguments"]),
                TwoArgumentsCosting::constant_cost(cost_map["bls12_381_G2_equal-cpu-arguments"]),
            ),

            bls12_381_g2_hash_to_group: TwoArgumentsCosting::new(
                TwoArgumentsCosting::constant_cost(cost_map["bls12_381_G2_hashToGroup-mem-arguments"]),
                TwoArgumentsCosting::linear_in_x(
                    cost_map["bls12_381_G2_hashToGroup-cpu-arguments-intercept"],
                    cost_map["bls12_381_G2_hashToGroup-cpu-arguments-slope"],
                ),
            ),

            bls12_381_g2_neg: OneArgumentCosting::new(
                OneArgumentCosting::constant_cost(cost_map["bls12_381_G2_neg-mem-arguments"]),
                OneArgumentCosting::constant_cost(cost_map["bls12_381_G2_neg-cpu-arguments"]),
            ),

            bls12_381_g2_scalar_mul: TwoArgumentsCosting::new(
                TwoArgumentsCosting::constant_cost(cost_map["bls12_381_G2_scalarMul-mem-arguments"]),
                TwoArgumentsCosting::linear_in_x(
                    cost_map["bls12_381_G2_scalarMul-cpu-arguments-intercept"],
                    cost_map["bls12_381_G2_scalarMul-cpu-arguments-slope"],
                ),
            ),

            bls12_381_g2_uncompress: OneArgumentCosting::new(
                OneArgumentCosting::constant_cost(cost_map["bls12_381_G2_uncompress-mem-arguments"]),
                OneArgumentCosting::constant_cost(cost_map["bls12_381_G2_uncompress-cpu-arguments"]),
            ),

            bls12_381_final_verify: TwoArgumentsCosting::new(
                TwoArgumentsCosting::constant_cost(cost_map["bls12_381_finalVerify-mem-arguments"]),
                TwoArgumentsCosting::constant_cost(cost_map["bls12_381_finalVerify-cpu-arguments"]),
            ),

            bls12_381_miller_loop: TwoArgumentsCosting::new(
                TwoArgumentsCosting::constant_cost(cost_map["bls12_381_millerLoop-mem-arguments"]),
                TwoArgumentsCosting::constant_cost(cost_map["bls12_381_millerLoop-cpu-arguments"]),
            ),

            bls12_381_mul_ml_result: TwoArgumentsCosting::new(
                TwoArgumentsCosting::constant_cost(cost_map["bls12_381_mulMlResult-mem-arguments"]),
                TwoArgumentsCosting::constant_cost(cost_map["bls12_381_mulMlResult-cpu-arguments"]),
            ),

            keccak_256: OneArgumentCosting::new(
                OneArgumentCosting::constant_cost(cost_map["keccak_256-mem-arguments"]),
                OneArgumentCosting::linear_cost(
                    cost_map["keccak_256-cpu-arguments-intercept"],
                    cost_map["keccak_256-cpu-arguments-slope"],
                ),
            ),

            blake2b_224: OneArgumentCosting::new(
                OneArgumentCosting::constant_cost(cost_map["blake2b_224-mem-arguments"]),
                OneArgumentCosting::linear_cost(
                    cost_map["blake2b_224-cpu-arguments-intercept"],
                    cost_map["blake2b_224-cpu-arguments-slope"],
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

            and_byte_string: ThreeArgumentsCosting::new(
                ThreeArgumentsCosting::linear_in_max_y_z(
                    cost_map["andByteString-memory-arguments-intercept"],
                    cost_map["andByteString-memory-arguments-slope"],
                ),
                ThreeArgumentsCosting::linear_in_y_and_z(
                    cost_map["andByteString-cpu-arguments-intercept"],
                    cost_map["andByteString-cpu-arguments-slope1"],
                    cost_map["andByteString-cpu-arguments-slope2"],
                ),
            ),

            or_byte_string: ThreeArgumentsCosting::new(
                ThreeArgumentsCosting::linear_in_max_y_z(
                    cost_map["orByteString-memory-arguments-intercept"],
                    cost_map["orByteString-memory-arguments-slope"],
                ),
                ThreeArgumentsCosting::linear_in_y_and_z(
                    cost_map["orByteString-cpu-arguments-intercept"],
                    cost_map["orByteString-cpu-arguments-slope1"],
                    cost_map["orByteString-cpu-arguments-slope2"],
                ),
            ),

            xor_byte_string: ThreeArgumentsCosting::new(
                ThreeArgumentsCosting::linear_in_max_y_z(
                    cost_map["xorByteString-memory-arguments-intercept"],
                    cost_map["xorByteString-memory-arguments-slope"],
                ),
                ThreeArgumentsCosting::linear_in_y_and_z(
                    cost_map["xorByteString-cpu-arguments-intercept"],
                    cost_map["xorByteString-cpu-arguments-slope1"],
                    cost_map["xorByteString-cpu-arguments-slope2"],
                ),
            ),

            complement_byte_string: OneArgumentCosting::new(
                OneArgumentCosting::linear_cost(
                    cost_map["complementByteString-memory-arguments-intercept"],
                    cost_map["complementByteString-memory-arguments-slope"],
                ),
                OneArgumentCosting::linear_cost(
                    cost_map["complementByteString-cpu-arguments-intercept"],
                    cost_map["complementByteString-cpu-arguments-slope"],
                ),
            ),

            read_bit: TwoArgumentsCosting::new(
                TwoArgumentsCosting::constant_cost(cost_map["readBit-memory-arguments"]),
                TwoArgumentsCosting::constant_cost(cost_map["readBit-cpu-arguments"]),
            ),

            write_bits: ThreeArgumentsCosting::new(
                ThreeArgumentsCosting::linear_in_x(
                    cost_map["writeBits-memory-arguments-intercept"],
                    cost_map["writeBits-memory-arguments-slope"],
                ),
                ThreeArgumentsCosting::linear_in_y(
                    cost_map["writeBits-cpu-arguments-intercept"],
                    cost_map["writeBits-cpu-arguments-slope"],
                ),
            ),

            replicate_byte: TwoArgumentsCosting::new(
                TwoArgumentsCosting::linear_in_x(
                    cost_map["replicateByte-memory-arguments-intercept"],
                    cost_map["replicateByte-memory-arguments-slope"],
                ),
                TwoArgumentsCosting::linear_in_x(
                    cost_map["replicateByte-cpu-arguments-intercept"],
                    cost_map["replicateByte-cpu-arguments-slope"],
                ),
            ),

            shift_byte_string: TwoArgumentsCosting::new(
                TwoArgumentsCosting::linear_in_x(
                    cost_map["shiftByteString-memory-arguments-intercept"],
                    cost_map["shiftByteString-memory-arguments-slope"],
                ),
                TwoArgumentsCosting::linear_in_x(
                    cost_map["shiftByteString-cpu-arguments-intercept"],
                    cost_map["shiftByteString-cpu-arguments-slope"],
                ),
            ),

            rotate_byte_string: TwoArgumentsCosting::new(
                TwoArgumentsCosting::linear_in_x(
                    cost_map["rotateByteString-memory-arguments-intercept"],
                    cost_map["rotateByteString-memory-arguments-slope"],
                ),
                TwoArgumentsCosting::linear_in_x(
                    cost_map["rotateByteString-cpu-arguments-intercept"],
                    cost_map["rotateByteString-cpu-arguments-slope"],
                ),
            ),

            count_set_bits: OneArgumentCosting::new(
                OneArgumentCosting::constant_cost(cost_map["countSetBits-memory-arguments"]),
                OneArgumentCosting::linear_cost(
                    cost_map["countSetBits-cpu-arguments-intercept"],
                    cost_map["countSetBits-cpu-arguments-slope"],
                ),
            ),

            find_first_set_bit: OneArgumentCosting::new(
                OneArgumentCosting::constant_cost(cost_map["findFirstSetBit-memory-arguments"]),
                OneArgumentCosting::linear_cost(
                    cost_map["findFirstSetBit-cpu-arguments-intercept"],
                    cost_map["findFirstSetBit-cpu-arguments-slope"],
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
            bls12_381_g1_multi_scalar_mul: TwoArgumentsCosting::new(
                TwoArgumentsCosting::constant_cost(cost_map["bls12_381_G1_multiScalarMul-mem-arguments"]),
                TwoArgumentsCosting::linear_in_x(
                    cost_map["bls12_381_G1_multiScalarMul-cpu-arguments-intercept"],
                    cost_map["bls12_381_G1_multiScalarMul-cpu-arguments-slope"],
                ),
            ),
            bls12_381_g2_multi_scalar_mul: TwoArgumentsCosting::new(
                TwoArgumentsCosting::constant_cost(cost_map["bls12_381_G2_multiScalarMul-mem-arguments"]),
                TwoArgumentsCosting::linear_in_x(
                    cost_map["bls12_381_G2_multiScalarMul-cpu-arguments-intercept"],
                    cost_map["bls12_381_G2_multiScalarMul-cpu-arguments-slope"],
                ),
            ),
            insert_coin: OneArgumentCosting::new(
                OneArgumentCosting::linear_cost(
                    cost_map["insert_coin-mem-arguments-intercept"],
                    cost_map["insert_coin-mem-arguments-slope"],
                ),
                OneArgumentCosting::linear_cost(
                    cost_map["insert_coin-cpu-arguments-intercept"],
                    cost_map["insert_coin-cpu-arguments-slope"],
                ),
            ),
            lookup_coin: ThreeArgumentsCosting::new(
                ThreeArgumentsCosting::constant_cost(cost_map["lookup_coin-mem-arguments"]),
                ThreeArgumentsCosting::linear_in_z(
                    cost_map["lookup_coin-cpu-arguments-intercept"],
                    cost_map["lookup_coin-cpu-arguments-slope"],
                ),
            ),
            union_value: TwoArgumentsCosting::new(
                TwoArgumentsCosting::added_sizes(
                    cost_map["union_value-mem-arguments-intercept"],
                    cost_map["union_value-mem-arguments-slope"],
                ),
                TwoArgumentsCosting::with_interaction(
                    cost_map["union_value-cpu-arguments-c00"],
                    cost_map["union_value-cpu-arguments-c10"],
                    cost_map["union_value-cpu-arguments-c01"],
                    cost_map["union_value-cpu-arguments-c11"],
                ),
            ),
            value_contains: TwoArgumentsCosting::new(
                TwoArgumentsCosting::constant_cost(cost_map["value_contains-mem-arguments"]),
                TwoArgumentsCosting::const_above_diagonal_into_linear_in_x_and_y(
                    cost_map["value_contains-cpu-arguments-constant"],
                    cost_map["value_contains-cpu-arguments-model-arguments-intercept"],
                    cost_map["value_contains-cpu-arguments-model-arguments-slope1"],
                    cost_map["value_contains-cpu-arguments-model-arguments-slope2"],
                ),
            ),
            value_data: OneArgumentCosting::new(
                OneArgumentCosting::linear_cost(
                    cost_map["value_data-mem-arguments-intercept"],
                    cost_map["value_data-mem-arguments-slope"],
                ),
                OneArgumentCosting::linear_cost(
                    cost_map["value_data-cpu-arguments-intercept"],
                    cost_map["value_data-cpu-arguments-slope"],
                ),
            ),
            un_value_data: OneArgumentCosting::new(
                OneArgumentCosting::linear_cost(
                    cost_map["un_value_data-mem-arguments-intercept"],
                    cost_map["un_value_data-mem-arguments-slope"],
                ),
                OneArgumentCosting::quadratic_cost(
                    cost_map["un_value_data-cpu-arguments-c0"],
                    cost_map["un_value_data-cpu-arguments-c1"],
                    cost_map["un_value_data-cpu-arguments-c2"],
                ),
            ),
            scale_value: TwoArgumentsCosting::new(
                TwoArgumentsCosting::linear_in_y(
                    cost_map["scale_value-mem-arguments-intercept"],
                    cost_map["scale_value-mem-arguments-slope"],
                ),
                TwoArgumentsCosting::linear_in_y(
                    cost_map["scale_value-cpu-arguments-intercept"],
                    cost_map["scale_value-cpu-arguments-slope"],
                ),
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
            DefaultFunction::Blake2b_224 => {
                Some(ExBudget::new(self.blake2b_224.mem.cost([args[0]]), self.blake2b_224.cpu.cost([args[0]])))
            }
            DefaultFunction::Blake2b_256 => {
                Some(ExBudget::new(self.blake2b_256.mem.cost([args[0]]), self.blake2b_256.cpu.cost([args[0]])))
            }
            DefaultFunction::Keccak_256 => {
                Some(ExBudget::new(self.keccak_256.mem.cost([args[0]]), self.keccak_256.cpu.cost([args[0]])))
            }
            DefaultFunction::VerifyEd25519Signature => Some(ExBudget::new(
                self.verify_ed25519_signature.mem.cost([args[0], args[1], args[2]]),
                self.verify_ed25519_signature.cpu.cost([args[0], args[1], args[2]]),
            )),
            DefaultFunction::VerifyEcdsaSecp256k1Signature => Some(ExBudget::new(
                self.verify_ecdsa_secp256k1_signature.mem.cost([args[0], args[1], args[2]]),
                self.verify_ecdsa_secp256k1_signature.cpu.cost([args[0], args[1], args[2]]),
            )),
            DefaultFunction::VerifySchnorrSecp256k1Signature => Some(ExBudget::new(
                self.verify_schnorr_secp256k1_signature.mem.cost([args[0], args[1], args[2]]),
                self.verify_schnorr_secp256k1_signature.cpu.cost([args[0], args[1], args[2]]),
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
            DefaultFunction::SerialiseData => {
                Some(ExBudget::new(self.serialise_data.mem.cost([args[0]]), self.serialise_data.cpu.cost([args[0]])))
            }
            DefaultFunction::Bls12_381_G1_Add => Some(ExBudget::new(
                self.bls12_381_g1_add.mem.cost([args[0], args[1]]),
                self.bls12_381_g1_add.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::Bls12_381_G1_Neg => Some(ExBudget::new(
                self.bls12_381_g1_neg.mem.cost([args[0]]),
                self.bls12_381_g1_neg.cpu.cost([args[0]]),
            )),
            DefaultFunction::Bls12_381_G1_ScalarMul => Some(ExBudget::new(
                self.bls12_381_g1_scalar_mul.mem.cost([args[0], args[1]]),
                self.bls12_381_g1_scalar_mul.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::Bls12_381_G1_Equal => Some(ExBudget::new(
                self.bls12_381_g1_equal.mem.cost([args[0], args[1]]),
                self.bls12_381_g1_equal.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::Bls12_381_G1_Compress => Some(ExBudget::new(
                self.bls12_381_g1_compress.mem.cost([args[0]]),
                self.bls12_381_g1_compress.cpu.cost([args[0]]),
            )),
            DefaultFunction::Bls12_381_G1_Uncompress => Some(ExBudget::new(
                self.bls12_381_g1_uncompress.mem.cost([args[0]]),
                self.bls12_381_g1_uncompress.cpu.cost([args[0]]),
            )),
            DefaultFunction::Bls12_381_G1_HashToGroup => Some(ExBudget::new(
                self.bls12_381_g1_hash_to_group.mem.cost([args[0], args[1]]),
                self.bls12_381_g1_hash_to_group.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::Bls12_381_G2_Add => Some(ExBudget::new(
                self.bls12_381_g2_add.mem.cost([args[0], args[1]]),
                self.bls12_381_g2_add.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::Bls12_381_G2_Neg => Some(ExBudget::new(
                self.bls12_381_g2_neg.mem.cost([args[0]]),
                self.bls12_381_g2_neg.cpu.cost([args[0]]),
            )),
            DefaultFunction::Bls12_381_G2_ScalarMul => Some(ExBudget::new(
                self.bls12_381_g2_scalar_mul.mem.cost([args[0], args[1]]),
                self.bls12_381_g2_scalar_mul.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::Bls12_381_G2_Equal => Some(ExBudget::new(
                self.bls12_381_g2_equal.mem.cost([args[0], args[1]]),
                self.bls12_381_g2_equal.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::Bls12_381_G2_Compress => Some(ExBudget::new(
                self.bls12_381_g2_compress.mem.cost([args[0]]),
                self.bls12_381_g2_compress.cpu.cost([args[0]]),
            )),
            DefaultFunction::Bls12_381_G2_Uncompress => Some(ExBudget::new(
                self.bls12_381_g2_uncompress.mem.cost([args[0]]),
                self.bls12_381_g2_uncompress.cpu.cost([args[0]]),
            )),
            DefaultFunction::Bls12_381_G2_HashToGroup => Some(ExBudget::new(
                self.bls12_381_g2_hash_to_group.mem.cost([args[0], args[1]]),
                self.bls12_381_g2_hash_to_group.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::Bls12_381_MillerLoop => Some(ExBudget::new(
                self.bls12_381_miller_loop.mem.cost([args[0], args[1]]),
                self.bls12_381_miller_loop.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::Bls12_381_MulMlResult => Some(ExBudget::new(
                self.bls12_381_mul_ml_result.mem.cost([args[0], args[1]]),
                self.bls12_381_mul_ml_result.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::Bls12_381_FinalVerify => Some(ExBudget::new(
                self.bls12_381_final_verify.mem.cost([args[0], args[1]]),
                self.bls12_381_final_verify.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::IntegerToByteString => Some(ExBudget::new(
                self.integer_to_byte_string.mem.cost([args[0], args[1], args[2]]),
                self.integer_to_byte_string.cpu.cost([args[0], args[1], args[2]]),
            )),
            DefaultFunction::ByteStringToInteger => Some(ExBudget::new(
                self.byte_string_to_integer.mem.cost([args[0], args[1]]),
                self.byte_string_to_integer.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::AndByteString => Some(ExBudget::new(
                self.and_byte_string.mem.cost([args[0], args[1], args[2]]),
                self.and_byte_string.cpu.cost([args[0], args[1], args[2]]),
            )),
            DefaultFunction::OrByteString => Some(ExBudget::new(
                self.or_byte_string.mem.cost([args[0], args[1], args[2]]),
                self.or_byte_string.cpu.cost([args[0], args[1], args[2]]),
            )),
            DefaultFunction::XorByteString => Some(ExBudget::new(
                self.xor_byte_string.mem.cost([args[0], args[1], args[2]]),
                self.xor_byte_string.cpu.cost([args[0], args[1], args[2]]),
            )),
            DefaultFunction::ComplementByteString => Some(ExBudget::new(
                self.complement_byte_string.mem.cost([args[0]]),
                self.complement_byte_string.cpu.cost([args[0]]),
            )),
            DefaultFunction::ReadBit => Some(ExBudget::new(
                self.read_bit.mem.cost([args[0], args[1]]),
                self.read_bit.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::WriteBits => Some(ExBudget::new(
                self.write_bits.mem.cost([args[0], args[1], args[2]]),
                self.write_bits.cpu.cost([args[0], args[1], args[2]]),
            )),
            DefaultFunction::ReplicateByte => Some(ExBudget::new(
                self.replicate_byte.mem.cost([args[0], args[1]]),
                self.replicate_byte.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::ShiftByteString => Some(ExBudget::new(
                self.shift_byte_string.mem.cost([args[0], args[1]]),
                self.shift_byte_string.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::RotateByteString => Some(ExBudget::new(
                self.rotate_byte_string.mem.cost([args[0], args[1]]),
                self.rotate_byte_string.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::CountSetBits => {
                Some(ExBudget::new(self.count_set_bits.mem.cost([args[0]]), self.count_set_bits.cpu.cost([args[0]])))
            }
            DefaultFunction::FindFirstSetBit => Some(ExBudget::new(
                self.find_first_set_bit.mem.cost([args[0]]),
                self.find_first_set_bit.cpu.cost([args[0]]),
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
            DefaultFunction::Bls12_381_G1_MultiScalarMul => Some(ExBudget::new(
                self.bls12_381_g1_multi_scalar_mul.mem.cost([args[0], args[1]]),
                self.bls12_381_g1_multi_scalar_mul.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::Bls12_381_G2_MultiScalarMul => Some(ExBudget::new(
                self.bls12_381_g2_multi_scalar_mul.mem.cost([args[0], args[1]]),
                self.bls12_381_g2_multi_scalar_mul.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::InsertCoin => {
                Some(ExBudget::new(self.insert_coin.mem.cost([args[0]]), self.insert_coin.cpu.cost([args[0]])))
            }
            DefaultFunction::LookupCoin => Some(ExBudget::new(
                self.lookup_coin.mem.cost([args[0], args[1], args[2]]),
                self.lookup_coin.cpu.cost([args[0], args[1], args[2]]),
            )),
            DefaultFunction::UnionValue => Some(ExBudget::new(
                self.union_value.mem.cost([args[0], args[1]]),
                self.union_value.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::ValueContains => Some(ExBudget::new(
                self.value_contains.mem.cost([args[0], args[1]]),
                self.value_contains.cpu.cost([args[0], args[1]]),
            )),
            DefaultFunction::ValueData => {
                Some(ExBudget::new(self.value_data.mem.cost([args[0]]), self.value_data.cpu.cost([args[0]])))
            }
            DefaultFunction::UnValueData => {
                Some(ExBudget::new(self.un_value_data.mem.cost([args[0]]), self.un_value_data.cpu.cost([args[0]])))
            }
            DefaultFunction::ScaleValue => Some(ExBudget::new(
                self.scale_value.mem.cost([args[0], args[1]]),
                self.scale_value.cpu.cost([args[0], args[1]]),
            )),
        }
    }
}
