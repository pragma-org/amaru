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

use std::{env, ffi::OsStr, fs, path::PathBuf};

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

fn main() {
    let crate_root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let conformance_root = crate_root.join("tests").join("conformance");

    println!("cargo:rerun-if-changed={}", conformance_root.display());

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());

    fs::write(out_dir.join("generated_tests.rs"), generate_conformance_tests(&conformance_root))
        .unwrap_or_else(|e| panic!("failed to generate conformance tests: {e}"));
}

fn generate_conformance_tests(dir_path: &PathBuf) -> String {
    let mut tests = String::new();

    for entry in WalkDir::new(dir_path).into_iter().filter_map(Result::ok) {
        let path = entry.path();

        if !path.file_name().and_then(OsStr::to_str).is_some_and(|name| name.ends_with(".uplc")) {
            continue;
        }

        let test_name = path
            .strip_prefix(dir_path)
            .unwrap()
            .parent()
            .unwrap()
            .to_str()
            .unwrap()
            .replace(|c: char| !c.is_alphanumeric(), "_")
            .to_lowercase();

        let text_path = path.to_str().unwrap().replace('\\', "/");
        let text = format!("include_str!(\"{text_path}\")");

        let flat_path = format!("{}.flat", text_path.strip_suffix(".uplc").unwrap());
        let flat = format!("include_str!(\"{flat_path}\")",);

        let (expected_text, expected_flat) = if OUT_OF_BUDGET_TESTS.contains(&test_name.as_str()) {
            (OUT_OF_BUDGET.to_string(), OUT_OF_BUDGET.to_string())
        } else {
            (format!("include_str!(\"{text_path}.expected\")"), format!("include_str!(\"{flat_path}.expected\")"))
        };

        let budget = format!("include_str!(\"{text_path}.budget.expected\")");

        if !tests.is_empty() {
            tests.push('\n');
        }

        tests.push_str(&formatdoc! {r#"
            #[test]
            fn {test_name}() {{
                run_conformance(
                    {text},
                    {expected_text},
                    {flat},
                    {expected_flat},
                    {budget},
                );
            }}
        "#});
    }

    tests
}
