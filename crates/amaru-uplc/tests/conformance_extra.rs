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
    protocol_version::{PROTOCOL_VERSION_10, PROTOCOL_VERSION_11},
};
use amaru_uplc::{
    arena::Arena,
    machine::{CostModel, ExBudget},
    syn::parse_program,
};

fn run_conformance_with_params(
    plutus_version: PlutusVersion,
    protocol_version: ProtocolVersion,
    file_contents: &str,
    expected_output: &str,
    expected_budget: &str,
) {
    let file_contents = &file_contents.replace("\r\n", "\n");
    let expected_output = &expected_output.replace("\r\n", "\n");
    let expected_budget = &expected_budget.replace("\r\n", "\n");

    let arena = Arena::new();

    let v3_costs: &[i64] = &[
        100788, 420, 1, 1, 1000, 173, 0, 1, 1000, 59957, 4, 1, 11183, 32, 201305, 8356, 4, 16000, 100, 16000, 100,
        16000, 100, 16000, 100, 16000, 100, 16000, 100, 100, 100, 16000, 100, 94375, 32, 132994, 32, 61462, 4, 72010,
        178, 0, 1, 22151, 32, 91189, 769, 4, 2, 85848, 123203, 7305, -900, 1716, 549, 57, 85848, 0, 1, 1, 1000, 42921,
        4, 2, 24548, 29498, 38, 1, 898148, 27279, 1, 51775, 558, 1, 39184, 1000, 60594, 1, 141895, 32, 83150, 32,
        15299, 32, 76049, 1, 13169, 4, 22100, 10, 28999, 74, 1, 28999, 74, 1, 43285, 552, 1, 44749, 541, 1, 33852, 32,
        68246, 32, 72362, 32, 7243, 32, 7391, 32, 11546, 32, 85848, 123203, 7305, -900, 1716, 549, 57, 85848, 0, 1,
        90434, 519, 0, 1, 74433, 32, 85848, 123203, 7305, -900, 1716, 549, 57, 85848, 0, 1, 1, 85848, 123203, 7305,
        -900, 1716, 549, 57, 85848, 0, 1, 955506, 213312, 0, 2, 270652, 22588, 4, 1457325, 64566, 4, 20467, 1, 4, 0,
        141992, 32, 100788, 420, 1, 1, 81663, 32, 59498, 32, 20142, 32, 24588, 32, 20744, 32, 25933, 32, 24623, 32,
        43053543, 10, 53384111, 14333, 10, 43574283, 26308, 10, 16000, 100, 16000, 100, 962335, 18, 2780678, 6, 442008,
        1, 52538055, 3756, 18, 267929, 18, 76433006, 8868, 18, 52948122, 18, 1995836, 36, 3227919, 12, 901022, 1,
        166917843, 4307, 36, 284546, 36, 158221314, 26549, 36, 74698472, 36, 333849714, 1, 254006273, 72, 2174038, 72,
        2261318, 64571, 4, 207616, 8310, 4, 1293828, 28716, 63, 0, 1, 1006041, 43623, 251, 0, 1, 100181, 726, 719, 0,
        1, 100181, 726, 719, 0, 1, 100181, 726, 719, 0, 1, 107878, 680, 0, 1, 95336, 1, 281145, 18848, 0, 1, 180194,
        159, 1, 1, 158519, 8942, 0, 1, 159378, 8813, 0, 1, 107490, 3298, 1, 106057, 655, 1, 1964219, 24520, 3,
    ];

    let Ok(program) = parse_program(&arena, file_contents, protocol_version).into_result() else {
        pretty_assertions::assert_eq!("parse error", expected_output.trim_end());
        pretty_assertions::assert_eq!("parse error", expected_budget.trim_end());
        return;
    };

    let costs: &[i64] = match plutus_version {
        PlutusVersion::V1 => &CostModel::DEFAULT_V1,
        PlutusVersion::V2 => &CostModel::DEFAULT_V2,
        PlutusVersion::V3 if protocol_version >= PROTOCOL_VERSION_11 => &CostModel::DEFAULT_V3,
        PlutusVersion::V3 => v3_costs,
    };

    let result = program.eval(&arena, CostModel::new(plutus_version, protocol_version, costs), ExBudget::default());

    let info = result.info;

    let Ok(term) = result.term else {
        pretty_assertions::assert_eq!("evaluation failure", expected_output.trim_end());
        pretty_assertions::assert_eq!("evaluation failure", expected_budget.trim_end());
        return;
    };

    #[expect(clippy::unwrap_used)]
    let expected = parse_program(&arena, expected_output, protocol_version).into_result().unwrap();

    pretty_assertions::assert_eq!(expected.term, term);

    let consumed_budget = format!("({{cpu: {}\n| mem: {}}})", info.consumed_budget.cpu, info.consumed_budget.mem);

    pretty_assertions::assert_eq!(consumed_budget, expected_budget.trim_end());
}

macro_rules! regression_case {
    ($name:ident, $path:literal) => {
        regression_case!($name, $path, PlutusVersion::V3, PROTOCOL_VERSION_10);
    };
    ($name:ident, $path:literal, $plutus_version:expr, $protocol_version:expr) => {
        #[test]
        fn $name() {
            run_conformance_with_params(
                $plutus_version,
                $protocol_version,
                include_str!($path),
                include_str!(concat!($path, ".expected")),
                include_str!(concat!($path, ".budget.expected")),
            );
        }
    };
}

regression_case!(
    builtin_semantics_consbytestring_v2_negative_wraps_pv10_regression,
    "conformance_extra/textual/builtin/semantics/consByteString/v2-negative-wraps/v2-negative-wraps.uplc",
    PlutusVersion::V2,
    PROTOCOL_VERSION_10
);
regression_case!(
    builtin_semantics_consbytestring_v2_negative_wraps_pv11_regression,
    "conformance_extra/textual/builtin/semantics/consByteString/v2-negative-wraps/v2-negative-wraps.uplc",
    PlutusVersion::V2,
    PROTOCOL_VERSION_11
);
regression_case!(
    builtin_semantics_constrdata_v3_negative_tag_regression,
    "conformance_extra/textual/builtin/semantics/constrData/v3-negative-tag/v3-negative-tag.uplc",
    PlutusVersion::V3,
    PROTOCOL_VERSION_11
);
regression_case!(
    builtin_semantics_constrdata_v3_tag_above_word64_regression,
    "conformance_extra/textual/builtin/semantics/constrData/v3-tag-above-word64/v3-tag-above-word64.uplc",
    PlutusVersion::V3,
    PROTOCOL_VERSION_11
);
regression_case!(
    builtin_semantics_divideinteger_v3_below_diagonal_constant_regression,
    "conformance_extra/textual/builtin/semantics/divideInteger/v3-below-diagonal-constant/v3-below-diagonal-constant.uplc"
);
regression_case!(
    builtin_semantics_divideinteger_v3_diagonal_c11_regression,
    "conformance_extra/textual/builtin/semantics/divideInteger/v3-diagonal-c11/v3-diagonal-c11.uplc"
);
regression_case!(
    builtin_semantics_indexarray_v3_index_beyond_i128_regression,
    "conformance_extra/textual/builtin/semantics/indexArray/v3-index-beyond-i128/v3-index-beyond-i128.uplc",
    PlutusVersion::V3,
    PROTOCOL_VERSION_11
);
regression_case!(
    builtin_semantics_indexarray_v3_negative_index_beyond_i128_regression,
    "conformance_extra/textual/builtin/semantics/indexArray/v3-negative-index-beyond-i128/v3-negative-index-beyond-i128.uplc",
    PlutusVersion::V3,
    PROTOCOL_VERSION_11
);
regression_case!(
    builtin_semantics_indexarray_v3_index_2pow64_plus_one_regression,
    "conformance_extra/textual/builtin/semantics/indexArray/v3-index-2pow64-plus-one/v3-index-2pow64-plus-one.uplc",
    PlutusVersion::V3,
    PROTOCOL_VERSION_11
);
regression_case!(
    builtin_semantics_indexbytestring_v3_index_beyond_i128_regression,
    "conformance_extra/textual/builtin/semantics/indexByteString/v3-index-beyond-i128/v3-index-beyond-i128.uplc"
);
regression_case!(
    builtin_semantics_indexbytestring_v3_negative_index_beyond_i128_regression,
    "conformance_extra/textual/builtin/semantics/indexByteString/v3-negative-index-beyond-i128/v3-negative-index-beyond-i128.uplc"
);
regression_case!(
    builtin_semantics_indexbytestring_v3_index_2pow64_plus_one_regression,
    "conformance_extra/textual/builtin/semantics/indexByteString/v3-index-2pow64-plus-one/v3-index-2pow64-plus-one.uplc"
);
regression_case!(
    builtin_semantics_listtoarray_v3_data_elements_regression,
    "conformance_extra/textual/builtin/semantics/listToArray/v3-data-elements/v3-data-elements.uplc",
    PlutusVersion::V3,
    PROTOCOL_VERSION_11
);
regression_case!(
    builtin_semantics_listtoarray_v3_string_elements_regression,
    "conformance_extra/textual/builtin/semantics/listToArray/v3-string-elements/v3-string-elements.uplc",
    PlutusVersion::V3,
    PROTOCOL_VERSION_11
);
regression_case!(
    builtin_semantics_listtoarray_v3_large_bytestring_elements_regression,
    "conformance_extra/textual/builtin/semantics/listToArray/v3-large-bytestring-elements/v3-large-bytestring-elements.uplc",
    PlutusVersion::V3,
    PROTOCOL_VERSION_11
);
regression_case!(
    builtin_semantics_listtoarray_v3_large_integer_elements_regression,
    "conformance_extra/textual/builtin/semantics/listToArray/v3-large-integer-elements/v3-large-integer-elements.uplc",
    PlutusVersion::V3,
    PROTOCOL_VERSION_11
);
regression_case!(
    builtin_semantics_listtoarray_v3_nested_list_elements_regression,
    "conformance_extra/textual/builtin/semantics/listToArray/v3-nested-list-elements/v3-nested-list-elements.uplc",
    PlutusVersion::V3,
    PROTOCOL_VERSION_11
);
regression_case!(
    builtin_semantics_modinteger_v3_below_diagonal_constant_regression,
    "conformance_extra/textual/builtin/semantics/modInteger/v3-below-diagonal-constant/v3-below-diagonal-constant.uplc"
);
regression_case!(
    builtin_semantics_droplist_v3_count_beyond_u64_regression,
    "conformance_extra/textual/builtin/semantics/dropList/v3-count-beyond-u64/v3-count-beyond-u64.uplc",
    PlutusVersion::V3,
    PROTOCOL_VERSION_11
);
regression_case!(
    builtin_semantics_droplist_v3_negative_count_beyond_u64_regression,
    "conformance_extra/textual/builtin/semantics/dropList/v3-negative-count-beyond-u64/v3-negative-count-beyond-u64.uplc",
    PlutusVersion::V3,
    PROTOCOL_VERSION_11
);
regression_case!(
    builtin_semantics_equalsbytestring_v3_off_diagonal_intercept_regression,
    "conformance_extra/textual/builtin/semantics/equalsByteString/v3-off-diagonal-intercept/v3-off-diagonal-intercept.uplc"
);
regression_case!(
    builtin_semantics_shiftbytestring_v3_left_shift_whole_byte_regression,
    "conformance_extra/textual/builtin/semantics/shiftByteString/v3-left-shift-whole-byte/v3-left-shift-whole-byte.uplc"
);
regression_case!(
    builtin_semantics_writebits_v3_multiple_indices_regression,
    "conformance_extra/textual/builtin/semantics/writeBits/v3-multiple-indices/v3-multiple-indices.uplc",
    PlutusVersion::V3,
    PROTOCOL_VERSION_11
);
regression_case!(
    builtin_semantics_writebits_v3_index_2pow64_regression,
    "conformance_extra/textual/builtin/semantics/writeBits/v3-index-2pow64/v3-index-2pow64.uplc",
    PlutusVersion::V3,
    PROTOCOL_VERSION_11
);
regression_case!(
    builtin_semantics_verifysignature_legacy_alias_test_vector_25_regression,
    "conformance_extra/textual/builtin/semantics/verifySignature/legacy-alias-test-vector-25/legacy-alias-test-vector-25.uplc"
);
