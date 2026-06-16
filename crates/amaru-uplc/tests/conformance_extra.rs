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

use amaru_uplc::{
    arena::Arena,
    machine::{ExBudget, PlutusVersion},
    syn::parse_program,
};

const PROTOCOL_VERSION: (u64, u64) = (11, 0);

const EXTRA_V3_COSTS: &[i64] = &[
    100788, 420, 1, 1, 1000, 173, 0, 1, 1000, 59957, 4, 1, 11183, 32, 201305, 8356, 4, 16000, 100, 16000, 100, 16000,
    100, 16000, 100, 16000, 100, 16000, 100, 100, 100, 16000, 100, 94375, 32, 132994, 32, 61462, 4, 72010, 178, 0, 1,
    22151, 32, 91189, 769, 4, 2, 85848, 123203, 7305, -900, 1716, 549, 57, 85848, 0, 1, 1, 1000, 42921, 4, 2, 24548,
    29498, 38, 1, 898148, 27279, 1, 51775, 558, 1, 39184, 1000, 60594, 1, 141895, 32, 83150, 32, 15299, 32, 76049, 1,
    13169, 4, 22100, 10, 28999, 74, 1, 28999, 74, 1, 43285, 552, 1, 44749, 541, 1, 33852, 32, 68246, 32, 72362, 32,
    7243, 32, 7391, 32, 11546, 32, 85848, 123203, 7305, -900, 1716, 549, 57, 85848, 0, 1, 90434, 519, 0, 1, 74433, 32,
    85848, 123203, 7305, -900, 1716, 549, 57, 85848, 0, 1, 1, 85848, 123203, 7305, -900, 1716, 549, 57, 85848, 0, 1,
    955506, 213312, 0, 2, 270652, 22588, 4, 1457325, 64566, 4, 20467, 1, 4, 0, 141992, 32, 100788, 420, 1, 1, 81663,
    32, 59498, 32, 20142, 32, 24588, 32, 20744, 32, 25933, 32, 24623, 32, 43053543, 10, 53384111, 14333, 10, 43574283,
    26308, 10, 16000, 100, 16000, 100, 962335, 18, 2780678, 6, 442008, 1, 52538055, 3756, 18, 267929, 18, 76433006,
    8868, 18, 52948122, 18, 1995836, 36, 3227919, 12, 901022, 1, 166917843, 4307, 36, 284546, 36, 158221314, 26549, 36,
    74698472, 36, 333849714, 1, 254006273, 72, 2174038, 72, 2261318, 64571, 4, 207616, 8310, 4, 1293828, 28716, 63, 0,
    1, 1006041, 43623, 251, 0, 1, 100181, 726, 719, 0, 1, 100181, 726, 719, 0, 1, 100181, 726, 719, 0, 1, 107878, 680,
    0, 1, 95336, 1, 281145, 18848, 0, 1, 180194, 159, 1, 1, 158519, 8942, 0, 1, 159378, 8813, 0, 1, 107490, 3298, 1,
    106057, 655, 1, 1964219, 24520, 3, // PV11 cost values (53 entries)
    607153, 231697, 53144, 0, 1, 116711, 1957, 4, 231883, 10, 1000, 24838, 7, 1, 232010, 32, 321837444, 25087669, 18,
    617887431, 67302824, 36, 356924, 18413, 45, 21, 219951, 9444, 1, 1000, 172116, 183150, 6, 24, 21, 213283, 618401,
    1998, 28258, 1, 1000, 38159, 2, 22, 1000, 95933, 1, 1, 11, 1000, 277577, 12, 21,
];

fn run_conformance_with_params(file_contents: &str, expected_output: &str, expected_budget: &str) {
    let file_contents = &file_contents.replace("\r\n", "\n");
    let expected_output = &expected_output.replace("\r\n", "\n");
    let expected_budget = &expected_budget.replace("\r\n", "\n");

    let arena = Arena::new();

    let Ok(program) = parse_program(&arena, file_contents, PROTOCOL_VERSION.0 as u32).into_result() else {
        pretty_assertions::assert_eq!("parse error", expected_output.trim_end());
        pretty_assertions::assert_eq!("parse error", expected_budget.trim_end());
        return;
    };

    let result =
        program.eval_with_params(&arena, PlutusVersion::V3, PROTOCOL_VERSION, EXTRA_V3_COSTS, ExBudget::default());

    let info = result.info;

    let Ok(term) = result.term else {
        pretty_assertions::assert_eq!("evaluation failure", expected_output.trim_end());
        pretty_assertions::assert_eq!("evaluation failure", expected_budget.trim_end());
        return;
    };

    let expected = parse_program(&arena, expected_output, PROTOCOL_VERSION.0 as u32).into_result().unwrap();

    pretty_assertions::assert_eq!(expected.term, term);

    let consumed_budget = format!("({{cpu: {}\n| mem: {}}})", info.consumed_budget.cpu, info.consumed_budget.mem);

    pretty_assertions::assert_eq!(consumed_budget, expected_budget.trim_end());
}

macro_rules! regression_case {
    ($name:ident, $path:literal) => {
        #[test]
        fn $name() {
            run_conformance_with_params(
                include_str!($path),
                include_str!(concat!($path, ".expected")),
                include_str!(concat!($path, ".budget.expected")),
            );
        }
    };
}

regression_case!(
    builtin_semantics_divideinteger_v3_below_diagonal_constant_regression,
    "conformance_extra/textual/builtin/semantics/divideInteger/v3-below-diagonal-constant/v3-below-diagonal-constant.uplc"
);
regression_case!(
    builtin_semantics_divideinteger_v3_diagonal_c11_regression,
    "conformance_extra/textual/builtin/semantics/divideInteger/v3-diagonal-c11/v3-diagonal-c11.uplc"
);
regression_case!(
    builtin_semantics_modinteger_v3_below_diagonal_constant_regression,
    "conformance_extra/textual/builtin/semantics/modInteger/v3-below-diagonal-constant/v3-below-diagonal-constant.uplc"
);
regression_case!(
    builtin_semantics_equalsbytestring_v3_off_diagonal_intercept_regression,
    "conformance_extra/textual/builtin/semantics/equalsByteString/v3-off-diagonal-intercept/v3-off-diagonal-intercept.uplc"
);
regression_case!(
    builtin_semantics_verifysignature_legacy_alias_test_vector_25_regression,
    "conformance_extra/textual/builtin/semantics/verifySignature/legacy-alias-test-vector-25/legacy-alias-test-vector-25.uplc"
);
