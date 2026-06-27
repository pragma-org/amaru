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

use amaru_kernel::{HasMajorVersion, protocol_version};
use amaru_uplc::{arena::Arena, syn::parse_program};

fn run_conformance_with_params(file_contents: &str, expected_output: &str, expected_budget: &str) {
    let file_contents = &file_contents.replace("\r\n", "\n");
    let expected_output = &expected_output.replace("\r\n", "\n");
    let expected_budget = &expected_budget.replace("\r\n", "\n");

    let arena = Arena::new();

    let Ok(program) = parse_program(&arena, file_contents, protocol_version::DEFAULT.major()).into_result() else {
        pretty_assertions::assert_eq!("parse error", expected_output.trim_end());
        pretty_assertions::assert_eq!("parse error", expected_budget.trim_end());
        return;
    };

    let result = program.eval_default(&arena);

    let info = result.info;

    let Ok(term) = result.term else {
        pretty_assertions::assert_eq!("evaluation failure", expected_output.trim_end());
        pretty_assertions::assert_eq!("evaluation failure", expected_budget.trim_end());
        return;
    };

    let expected = parse_program(&arena, expected_output, protocol_version::DEFAULT.major()).into_result().unwrap();

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
