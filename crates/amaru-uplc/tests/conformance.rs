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
    mem,
    sync::{Mutex, OnceLock},
};

use amaru_kernel::{PlutusVersion, ProtocolVersion, protocol_version};
use amaru_uplc::{
    arena::Arena,
    binder::DeBruijn,
    flat,
    flat::{FlatDecodeError, FlatEncodeError},
    machine::{CostModel, ExBudget, MachineError},
    syn::parse_program,
};
use serde_json::json;

static OUTPUT_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

static COST_MODEL_PV10_V1: OnceLock<Vec<i64>> = OnceLock::new();
static COST_MODEL_PV10_V2: OnceLock<Vec<i64>> = OnceLock::new();
static COST_MODEL_PV10_V3: OnceLock<Vec<i64>> = OnceLock::new();
static COST_MODEL_PV11_V1: OnceLock<Vec<i64>> = OnceLock::new();
static COST_MODEL_PV11_V2: OnceLock<Vec<i64>> = OnceLock::new();
static COST_MODEL_PV11_V3: OnceLock<Vec<i64>> = OnceLock::new();

macro_rules! cost_model {
    ($cache:ident, $path:literal, $length:literal) => {
        $cache.get_or_init(|| {
            parse_cost_model(
                $path,
                include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/conformance/cost-models/", $path)),
                $length,
            )
        })
    };
}

fn parse_cost_model(path: &str, contents: &str, expected_len: usize) -> Vec<i64> {
    let costs: Vec<i64> =
        serde_json::from_str(contents).unwrap_or_else(|error| panic!("invalid conformance cost model {path}: {error}"));
    assert_eq!(
        costs.len(),
        expected_len,
        "invalid conformance cost model {path}: expected {expected_len} entries, got {}",
        costs.len()
    );
    costs
}

fn conformance_cost_model(plutus_version: PlutusVersion, protocol_version: ProtocolVersion) -> CostModel {
    let costs = match (protocol_version.major(), plutus_version) {
        (10, PlutusVersion::V1) => cost_model!(COST_MODEL_PV10_V1, "protocol-version-10/v1.json", 332),
        (10, PlutusVersion::V2) => cost_model!(COST_MODEL_PV10_V2, "protocol-version-10/v2.json", 332),
        (10, PlutusVersion::V3) => cost_model!(COST_MODEL_PV10_V3, "protocol-version-10/v3.json", 350),
        (11, PlutusVersion::V1) => cost_model!(COST_MODEL_PV11_V1, "protocol-version-11/v1.json", 332),
        (11, PlutusVersion::V2) => cost_model!(COST_MODEL_PV11_V2, "protocol-version-11/v2.json", 332),
        (11, PlutusVersion::V3) => cost_model!(COST_MODEL_PV11_V3, "protocol-version-11/v3.json", 350),
        (major, version) => panic!("missing conformance cost model for protocol {major} and {version}"),
    };

    CostModel::new(plutus_version, protocol_version, costs)
}

pub fn budget(cpu: i64, mem: i64) -> String {
    format!("({{cpu: {cpu}\n| mem: {mem}}})")
}

const EVALUATION_FAILURE: &str = "evaluation failure";
const OUT_OF_BUDGET: &str = "out of budget";
const PARSE_ERROR: &str = "parse/decode error";

#[expect(clippy::too_many_arguments)]
fn run_conformance(
    test_name: &str,
    title: &str,
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
        report(test_name, title, group, &plutus_version.to_string(), &protocol_version.to_string(), PARSE_ERROR, None);
        assert_eq!(expected_output_text.trim(), PARSE_ERROR, "expected a parse/decode error marker");
        assert_eq!(expected_budget.trim(), PARSE_ERROR, "expected a parse/decode error marker");
        return;
    };

    let to_flat = |term| flat::encode(term).map(hex::encode).unwrap_or_else(|_| "?".to_string());

    let program_flat = match program_flat {
        None => None,
        Some(program_flat) => match flat::decode::<DeBruijn>(
            &arena,
            &hex::decode(program_flat.trim())
                .unwrap_or_else(|e| panic!("flat program ({program_flat}) isn't hex-encoded? {e}")),
            plutus_version,
            protocol_version,
        ) {
            Ok((program_flat, _)) => Some(Some(program_flat)),
            Err(FlatDecodeError::ProgramVersionNotAvailable { .. }) => {
                report(
                    test_name,
                    title,
                    group,
                    &plutus_version.to_string(),
                    &protocol_version.to_string(),
                    PARSE_ERROR,
                    None,
                );
                assert_eq!(expected_output_text.trim(), PARSE_ERROR, "expected a parse/decode error marker");
                assert_eq!(expected_budget.trim(), PARSE_ERROR, "expected a parse/decode error marker");
                return;
            }
            Err(_) if flat::encode(program).is_err_and(|e| matches!(e, FlatEncodeError::BlsElementNotSupported)) => {
                None
            }
            Err(e) => panic!(
                "failed to parse flat program after text program succeeded: {e};\nre-encoded text program={}",
                to_flat(program)
            ),
        },
    };

    if let Some(Some(program_flat)) = program_flat.as_ref() {
        pretty_assertions::assert_eq!(
            program,
            *program_flat,
            "text & flat programs are not equal; re-encoded text program={}",
            to_flat(program),
        );
    }

    let mut result =
        program.eval(&arena, conformance_cost_model(plutus_version, protocol_version), ExBudget::default());
    let info = mem::take(&mut result.info);

    let term = match result.into_term() {
        Ok(term) => term,
        Err(error) => {
            let actual =
                if matches!(error, MachineError::OutOfExError(_)) { OUT_OF_BUDGET } else { EVALUATION_FAILURE };
            report(test_name, title, group, &plutus_version.to_string(), &protocol_version.to_string(), actual, None);
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
        title,
        group,
        &plutus_version.to_string(),
        &protocol_version.to_string(),
        &expected_output_text,
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
    title: &str,
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
        "title": title,
        "group": group,
        "plutus_version": plutus_version,
        "protocol_version": protocol_version,
        "evaluation": evaluation_actual,
        "budget": actual_budget,
    });
    let mut output = OpenOptions::new().create(true).append(true).open(path).expect("open report output");
    writeln!(output, "{record}").expect("write report output");
}

include!(concat!(env!("OUT_DIR"), "/generated_tests.rs"));
