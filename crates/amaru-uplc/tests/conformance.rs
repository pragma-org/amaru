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

#![expect(clippy::panic, clippy::unwrap_used)]

use std::{
    fs::OpenOptions,
    io::Write,
    sync::{Mutex, OnceLock},
};

use amaru_kernel::{PlutusVersion, ProtocolVersion, protocol_version};
use amaru_uplc::{
    arena::Arena,
    binder::DeBruijn,
    flat,
    flat::FlatEncodeError,
    machine::{CostModel, ExBudget, MachineError},
    syn::parse_program,
};
use serde_json::json;

static OUTPUT_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

pub fn budget(cpu: i64, mem: i64) -> String {
    format!("({{cpu: {cpu}\n| mem: {mem}}})")
}

const EVALUATION_FAILURE: &str = "evaluation failure";
const OUT_OF_BUDGET: &str = "out of budget";
const PARSE_ERROR: &str = "parse error";

fn run_conformance(
    test_name: &str,
    group: &str,
    plutus_version: PlutusVersion,
    protocol_version: ProtocolVersion,
    program_text: &str,
    expected_output_text: &str,
    program_flat: Option<&str>,
    expected_output_flat: Option<&str>,
    expected_budget: &str,
) {
    let program_text = program_text.replace("\r\n", "\n");
    let expected_output_text = expected_output_text.replace("\r\n", "\n");
    let expected_output_flat = expected_output_flat.map(|value| value.replace("\r\n", "\n"));
    let expected_budget = expected_budget.replace("\r\n", "\n");

    let arena = Arena::new();
    let Ok(program) = parse_program(&arena, &program_text, protocol_version).into_result() else {
        report(test_name, group, &plutus_version.to_string(), &protocol_version.to_string(), PARSE_ERROR, None);
        pretty_assertions::assert_eq!(PARSE_ERROR, expected_output_text.trim());
        pretty_assertions::assert_eq!(PARSE_ERROR, expected_budget.trim());
        return;
    };

    let to_flat = |term| flat::encode(term).map(hex::encode).unwrap_or_else(|_| "?".to_string());

    let program_flat = program_flat.map(|program_flat| {
        match flat::decode::<DeBruijn>(
            &arena,
            &hex::decode(program_flat.trim())
                .unwrap_or_else(|e| panic!("flat program ({program_flat}) isn't hex-encoded? {e}")),
            plutus_version,
            protocol_version,
        ) {
            Ok((program_flat, _)) => Some(program_flat),
            Err(_) if flat::encode(program).is_err_and(|e| matches!(e, FlatEncodeError::BlsElementNotSupported)) => {
                None
            }
            Err(e) => panic!(
                "failed to parse flat program after text program succeeded: {e};\nre-encoded text program={}",
                to_flat(program)
            ),
        }
    });

    if let Some(Some(program_flat)) = program_flat.as_ref() {
        pretty_assertions::assert_eq!(
            program,
            *program_flat,
            "text & flat programs are not equal; re-encoded text program={}",
            to_flat(program),
        );
    }

    let result = if plutus_version == PlutusVersion::default() && protocol_version == protocol_version::DEFAULT {
        program.eval_default(&arena)
    } else {
        program.eval(&arena, CostModel::default_for(plutus_version, protocol_version), ExBudget::default())
    };
    let info = result.info;

    let term = match result.term {
        Ok(term) => term,
        Err(error) => {
            let actual =
                if matches!(error, MachineError::OutOfExError(_)) { OUT_OF_BUDGET } else { EVALUATION_FAILURE };
            report(test_name, group, &plutus_version.to_string(), &protocol_version.to_string(), actual, None);
            assert!(
                expected_output_text.trim() == EVALUATION_FAILURE
                    || (actual == OUT_OF_BUDGET && expected_output_text.trim() == OUT_OF_BUDGET),
                "expected failure but got {expected_output_text}"
            );
            if expected_output_flat.is_none() {
                pretty_assertions::assert_eq!(expected_budget.trim(), expected_output_text.trim());
            }
            if let Some(expected_output_flat) = expected_output_flat {
                pretty_assertions::assert_eq!(expected_output_flat.trim(), expected_output_text.trim());
            }
            return;
        }
    };

    let expected = parse_program(&arena, &expected_output_text, protocol_version).into_result().unwrap();
    pretty_assertions::assert_eq!(expected.term, term);

    let actual_budget = json!({
        "cpu": info.consumed_budget.cpu,
        "mem": info.consumed_budget.mem,
    });
    report(
        test_name,
        group,
        &plutus_version.to_string(),
        &protocol_version.to_string(),
        expected_output_text.trim(),
        Some(actual_budget),
    );

    if let (Some(expected_output_flat), Some(Some(_))) = (expected_output_flat, program_flat.as_ref()) {
        match flat::decode::<DeBruijn>(
            &arena,
            &hex::decode(expected_output_flat.trim())
                .unwrap_or_else(|e| panic!("flat expected output ({expected_output_flat}) isn't hex-encoded? {e}")),
            plutus_version,
            protocol_version,
        ) {
            Ok((expected_flat, _)) => pretty_assertions::assert_eq!(expected_flat.term, term),
            Err(_) if flat::encode(expected).is_err_and(|e| matches!(e, FlatEncodeError::BlsElementNotSupported)) => { /* ignore output which cannot be encoded as flat */
            }
            Err(e) => panic!(
                "failed to parse flat expected output: {e};\nre-encoded text expected output={}",
                to_flat(expected)
            ),
        }
    }

    let consumed_budget = format!("({{cpu: {}\n| mem: {}}})", info.consumed_budget.cpu, info.consumed_budget.mem);
    pretty_assertions::assert_eq!(consumed_budget, expected_budget.trim());
}

#[expect(clippy::expect_used)]
pub fn report(
    name: &str,
    group: &str,
    plutus_version: &str,
    protocol_version: &str,
    evaluation_actual: &str,
    actual_budget: Option<serde_json::Value>,
) {
    let Some(path) = std::env::var_os("UPLC_CONFORMANCE_REPORT") else {
        return;
    };

    let _guard = OUTPUT_LOCK.get_or_init(|| Mutex::new(())).lock().expect("report output lock poisoned");
    let record = json!({
        "id": name,
        "group": group,
        "plutus_version": plutus_version,
        "protocol_version": protocol_version,
        "evaluation": evaluation_actual,
        "budget": actual_budget,
    });
    let mut output = OpenOptions::new().create(true).append(true).open(path).expect("open report output");
    writeln!(output, "UPLC_CONFORMANCE_RESULT {record}").expect("write report output");
}

include!(concat!(env!("OUT_DIR"), "/generated_tests.rs"));
