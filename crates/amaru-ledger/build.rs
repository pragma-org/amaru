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

use std::{env, fs, path::Path};

fn main() {
    get_conformance_test_vectors();
}

#[allow(clippy::unwrap_used)]
fn get_conformance_test_vectors() {
    let test_dir = Path::new("tests/data/rules-conformance");
    if test_dir.exists() {
        println!("cargo:rerun-if-changed={}", test_dir.to_str().unwrap());
    }

    let mut files = Vec::new();
    visit_dirs(test_dir, &mut files);

    let out_dir = env::var("OUT_DIR").unwrap();
    let dest_path = Path::new(&out_dir).join("test_cases.rs");

    if files.is_empty() {
        println!(
            "cargo:warning=no conformance ledger test vectors found; unpack them in amaru-ledger/tests/data/conformance"
        );
    }

    let pparams_dir = test_dir.join("eras/conway/impl/dump/pparams-by-hash");

    let mut output = String::new();
    for path in files {
        if path.contains("pparams-by-hash") {
            continue;
        }

        output.push_str(&format!(
            "#[test_case::test_case(\"{}\", \"{}\")]\n",
            path,
            pparams_dir.to_str().unwrap()
        ));
    }

    output.push_str(
        r#"
pub fn rules_conformance_test_case(snapshot: &str, pparams_dir: &str) -> Result<(), Box<dyn std::error::Error>> {
    import_and_evaluate_vector(snapshot, pparams_dir)
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
