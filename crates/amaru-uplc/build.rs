// Copyright 2026 PRAGMA
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

#![expect(clippy::unwrap_used, clippy::panic)]

use std::{
    env,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
};

use indoc::formatdoc;
use walkdir::WalkDir;

/// The original conformance test suite from the IntersectMBO/plutus codebase are run with the VM
/// in "counting" mode. Some tests are purposely crafted to use a very large budget due to
/// arguments themselves being huge, but not actually inducing an actual execution cost.
///
/// This is the case, for instance, of the dropList builtin, which cost depends on the size of the
/// arguments, even if the list is empty (and thus, the drop virtually "free").
///
/// In the original conformance test suite, these tests pass successfully, but result in very large
/// budgets; in our case, these fail because they run out of budget (as they would in a real
/// scenario).
const OUT_OF_BUDGET_TESTS: &[&str] = &[
    "builtin_semantics_droplist_droplist_09",
    "builtin_semantics_droplist_droplist_10",
    "builtin_semantics_droplist_droplist_14",
    "builtin_semantics_droplist_droplist_15",
    "builtin_semantics_droplist_droplist_16",
];

const OUT_OF_BUDGET: &str = "\"out of budget\"";

#[derive(Clone, Copy)]
struct TestParameters {
    plutus_version: &'static str,
    protocol_version: &'static str,
}

fn main() {
    let crate_root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let conformance_root = crate_root.join("tests").join("conformance");

    println!("cargo:rerun-if-changed={}", conformance_root.display());

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());

    fs::write(out_dir.join("generated_tests.rs"), generate_conformance_tests(&conformance_root))
        .unwrap_or_else(|e| panic!("failed to generate conformance tests: {e}"));
}

fn generate_conformance_tests(dir_path: &Path) -> String {
    let mut tests = String::new();

    for entry in WalkDir::new(dir_path).into_iter().filter_map(Result::ok) {
        let path = entry.path();

        if !path.file_name().and_then(OsStr::to_str).is_some_and(|name| name.ends_with(".uplc")) {
            continue;
        }

        let relative_path = path.strip_prefix(dir_path).unwrap();
        let (protocol_version, scenario_path) = protocol_version(relative_path);
        let scenario_name = scenario_path
            .parent()
            .unwrap()
            .to_str()
            .unwrap()
            .replace(|c: char| !c.is_alphanumeric(), "_")
            .to_lowercase();
        let group = scenario_path
            .parent()
            .and_then(|parent| parent.parent())
            .unwrap_or_else(|| scenario_path.parent().unwrap())
            .to_string_lossy()
            .replace('\\', "/");

        let text_path = path.to_str().unwrap().replace('\\', "/");
        let text = format!("include_str!(\"{text_path}\")");

        let flat_path = path.with_extension("flat");
        let flat_path_string = flat_path.to_str().unwrap().replace('\\', "/");
        let flat =
            if flat_path.exists() { format!("Some(include_str!(\"{flat_path_string}\"))") } else { "None".to_string() };
        let expected_flat_path = path.with_extension("flat.expected");
        let expected_flat_path_string = expected_flat_path.to_str().unwrap().replace('\\', "/");
        let expected_flat = if expected_flat_path.exists() {
            format!("Some(include_str!(\"{expected_flat_path_string}\"))")
        } else {
            "None".to_string()
        };

        let expected_text = if OUT_OF_BUDGET_TESTS.contains(&scenario_name.as_str()) {
            OUT_OF_BUDGET.to_string()
        } else {
            format!("include_str!(\"{text_path}.expected\")")
        };
        let expected_flat = if OUT_OF_BUDGET_TESTS.contains(&scenario_name.as_str()) {
            format!("Some({OUT_OF_BUDGET})")
        } else {
            expected_flat
        };

        let budget_path = path.with_extension("budget");
        let budget_path_string = budget_path.to_str().unwrap().replace('\\', "/");
        let budget = format!("include_str!(\"{budget_path_string}\")");
        let configurations = test_parameters(scenario_path, protocol_version);

        for TestParameters { plutus_version, protocol_version } in configurations {
            let scenario_id = format!("{scenario_name}_{plutus_version}").to_lowercase();
            let test_name = format!("{scenario_id}_{protocol_version}").to_lowercase();
            if !tests.is_empty() {
                tests.push('\n');
            }
            tests.push_str(&formatdoc! {r#"
                #[test]
                fn {test_name}() {{
                    run_conformance(
                        "{scenario_id}",
                        "{group}",
                        PlutusVersion::{plutus_version},
                        protocol_version::{protocol_version},
                        {text},
                        {expected_text},
                        {flat},
                        {expected_flat},
                        {budget},
                    );
                }}
            "#});
        }
    }

    tests
}

fn test_parameters(scenario_path: &Path, protocol_version: &'static str) -> Vec<TestParameters> {
    let scenario = scenario_path.parent().unwrap().to_string_lossy();
    let plutus_versions: &[&str] = match scenario.as_ref() {
        // The same 1.1.0 program-version fixture is intentionally exercised as both V1 and V2.
        path if path.starts_with("program/version/v1-1-1-0-") => &["V1", "V2"],
        path if path.rsplit('/').next().is_some_and(|name| name.starts_with("v1-")) => &["V1"],
        path if path.rsplit('/').next().is_some_and(|name| name.starts_with("v2-")) => &["V2"],
        _ => &["V3"],
    };

    plutus_versions.iter().map(|&plutus_version| TestParameters { plutus_version, protocol_version }).collect()
}

fn protocol_version(path: &Path) -> (&'static str, &Path) {
    let component =
        path.components().next().unwrap_or_else(|| panic!("missing protocol-version directory in {}", path.display()));
    let protocol_directory = component.as_os_str().to_string_lossy();
    let protocol_version = match protocol_directory.as_ref() {
        "protocol-version-10" => "PROTOCOL_VERSION_10",
        "protocol-version-11" => "PROTOCOL_VERSION_11",
        other => panic!("unsupported conformance protocol directory: {other}"),
    };
    let scenario_path = path.strip_prefix(Path::new(component.as_os_str())).unwrap();
    (protocol_version, scenario_path)
}
