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
    machine::{ExBudget, PlutusVersion, default_v3_cost_model},
    syn::parse_program,
};

const PLUTUS_VERSION: PlutusVersion = PlutusVersion::V3;
const PROTOCOL_VERSION: (u64, u64) = (11, 0);

const PV11_COST_VALUES: &[i64] = &[
    607153, 231697, 53144, 0, 1, 116711, 1957, 4, 231883, 10, 1000, 24838, 7, 1, 232010, 32, 321837444, 25087669, 18,
    617887431, 67302824, 36, 356924, 18413, 45, 21, 219951, 9444, 1, 1000, 172116, 183150, 6, 24, 21, 213283, 618401,
    1998, 28258, 1, 1000, 38159, 2, 22, 1000, 95933, 1, 1, 11, 1000, 277577, 12, 21,
];

fn run_conformance(file_contents: &str, expected_output: &str, expected_budget: &str) {
    let file_contents = &file_contents.replace("\r\n", "\n");
    let expected_output = &expected_output.replace("\r\n", "\n");
    let expected_budget = &expected_budget.replace("\r\n", "\n");

    let arena = Arena::new();
    let mut costs = default_v3_cost_model();
    costs.extend(PV11_COST_VALUES);

    let Ok(program) = parse_program(&arena, file_contents, PROTOCOL_VERSION.0 as u32).into_result() else {
        pretty_assertions::assert_eq!("parse error", expected_output.as_str());
        pretty_assertions::assert_eq!("parse error", expected_budget.as_str());

        return;
    };

    let result = program.eval_with_params(&arena, PLUTUS_VERSION, PROTOCOL_VERSION, &costs, ExBudget::default());

    let info = result.info;

    let Ok(term) = result.term else {
        pretty_assertions::assert_eq!("evaluation failure", expected_output.as_str());
        pretty_assertions::assert_eq!("evaluation failure", expected_budget.as_str());

        return;
    };

    let expected = parse_program(&arena, expected_output, PROTOCOL_VERSION.0 as u32).into_result().unwrap();

    pretty_assertions::assert_eq!(expected.term, term);

    let consumed_budget = format!("({{cpu: {}\n| mem: {}}})", info.consumed_budget.cpu, info.consumed_budget.mem);

    pretty_assertions::assert_eq!(consumed_budget, expected_budget.as_str());
}

include!(concat!(env!("OUT_DIR"), "/generated_tests.rs"));
