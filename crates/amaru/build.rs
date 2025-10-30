// Copyright 2024 PRAGMA
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

use std::{env, fs, path::Path};

#[expect(clippy::expect_used)]
fn main() {
    built::write_built_file().expect("Failed to acquire build-time information");

    // Set the AMARU_NETWORK environment variable based on the value from the environment or default to "preprod"
    // This is necessary for the tests to run correctly, as they rely on the AMARU_NETWORK variable to be set at build time
    let network = env::var("AMARU_NETWORK").unwrap_or_else(|_| "preprod".into());
    println!("cargo:rerun-if-env-changed=AMARU_NETWORK");
    println!("cargo:rustc-env=AMARU_NETWORK={}", network);

    get_conformance_test_vectors();
}

#[allow(clippy::unwrap_used)]
fn get_conformance_test_vectors() {
    let test_dir = "../../cardano-blueprint/src/ledger/conformance-test-vectors/eras/conway";
    println!("cargo:rerun-if-changed={}", test_dir);

    let mut files = Vec::new();
    visit_dirs(Path::new(test_dir), &mut files);

    let out_dir = env::var("OUT_DIR").unwrap();
    let dest_path = Path::new(&out_dir).join("test_cases.rs");

    let mut output = String::new();
    for path in files {
        if path.contains("pparams-by-hash") {
            continue;
        }

        output.push_str(&format!("#[test_case(\"{}\")]\n", path));
    }

    output.push_str(
        r#"
#[allow(clippy::unwrap_used)]
pub fn compare_snapshot_test_case(snapshot: &str) {
    evaluate_vector(snapshot).unwrap()
}
"#,
    );

    fs::write(dest_path, output).unwrap();
}

fn visit_dirs(dir: &Path, files: &mut Vec<String>) {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                visit_dirs(&path, files);
            } else if path.is_file()
                && let Some(path_str) = path.to_str()
            {
                files.push(path_str.to_string());
            }
        }
    }
}
