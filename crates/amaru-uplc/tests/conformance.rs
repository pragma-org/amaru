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

use amaru_kernel::protocol_version;
use amaru_uplc::{arena::Arena, binder::DeBruijn, flat, flat::FlatEncodeError, syn::parse_program};

const EVALUATION_FAILURE: &str = "evaluation failure";
const OUT_OF_BUDGET: &str = "out of budget";
const PARSE_ERROR: &str = "parse error";

fn run_conformance(
    program_text: &str,
    expected_output_text: &str,
    program_flat: &str,
    expected_output_flat: &str,
    expected_budget: &str,
) {
    let program_text = &program_text.replace("\r\n", "\n");
    let expected_output_flat = &expected_output_flat.replace("\r\n", "\n");
    let expected_output_text = &expected_output_text.replace("\r\n", "\n");
    let expected_budget = &expected_budget.replace("\r\n", "\n");

    let arena = Arena::new();

    let Ok(program_text) = parse_program(&arena, program_text, protocol_version::DEFAULT).into_result() else {
        pretty_assertions::assert_eq!(
            PARSE_ERROR,
            expected_output_text,
            "(text) program should have failed to parse but succeeded"
        );
        return;
    };

    let to_flat = |text| flat::encode(text).map(hex::encode).unwrap_or_else(|_| "?".to_string());

    let program_flat = match flat::decode::<DeBruijn>(
        &arena,
        &hex::decode(program_flat.trim())
            .unwrap_or_else(|e| panic!("flat program ({program_flat}) isn't hex-encoded? {e}")),
        protocol_version::DEFAULT,
    ) {
        Ok((program_flat, _)) => Some(program_flat),
        Err(_) if flat::encode(program_text).is_err_and(|e| matches!(e, FlatEncodeError::BlsElementNotSupported)) => {
            None
            { /* ignore programs which cannot be encoded as flat */ }
        }
        Err(e) => panic!(
            "failed to parse flat program after text program succeeded: {e};\nre-encoded text program={}",
            to_flat(program_text)
        ),
    };

    if let Some(program_flat) = program_flat {
        pretty_assertions::assert_eq!(
            program_text,
            program_flat,
            "text & flat programs are not equal; re-encoded text program={}",
            to_flat(program_text),
        );
    }

    let program = program_text;

    let result = program.eval_default(&arena);

    let info = result.info;

    let Ok(term) = result.term else {
        assert!(
            expected_output_text == EVALUATION_FAILURE || expected_output_text == OUT_OF_BUDGET,
            "expected failure but got something else: {expected_output_text}"
        );
        assert_eq!(expected_output_flat.trim(), expected_output_text.trim());
        return;
    };

    let expected_text = parse_program(&arena, expected_output_text, protocol_version::DEFAULT).into_result().unwrap();
    pretty_assertions::assert_eq!(expected_text.term, term);

    if program_flat.is_some() {
        match flat::decode::<DeBruijn>(
            &arena,
            &hex::decode(expected_output_flat.trim())
                .unwrap_or_else(|e| panic!("flat expected output ({expected_output_flat}) isn't hex-encoded? {e}")),
            protocol_version::DEFAULT,
        ) {
            Ok((expected_flat, _)) => pretty_assertions::assert_eq!(expected_flat.term, term),
            Err(_)
                if flat::encode(expected_text).is_err_and(|e| matches!(e, FlatEncodeError::BlsElementNotSupported)) =>
            { /* ignore output which cannot be encoded as flat */ }
            Err(e) => {
                panic!(
                    "failed to parse flat expected output: {e};\nre-encoded text expected output={}",
                    to_flat(expected_text)
                )
            }
        }
    }

    let consumed_budget = format!("({{cpu: {}\n| mem: {}}})", info.consumed_budget.cpu, info.consumed_budget.mem);
    pretty_assertions::assert_eq!(consumed_budget, expected_budget.as_str());
}

include!(concat!(env!("OUT_DIR"), "/generated_tests.rs"));
