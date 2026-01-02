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

use std::{
    collections::BTreeMap,
    env, fs,
    io::Write as _,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};

fn main() -> Result<()> {
    get_conformance_test_vectors()
}

type FailuresTable = BTreeMap<String, String>;

fn get_conformance_test_vectors() -> Result<()> {
    let vectors_path = "tests/data/rules-conformance";
    println!("cargo:rerun-if-changed={vectors_path}");

    let failures_path = "tests/data/rules-conformance.failures.toml";
    println!("cargo:rerun-if-changed={failures_path}");
    let failures = match fs::read(failures_path) {
        Ok(bytes) => {
            toml::from_slice::<FailuresTable>(&bytes).context("could not parse failures file")?
        }
        Err(_) => FailuresTable::new(),
    };

    let out_dir = env::var("OUT_DIR").context("OUT_DIR not set")?;
    let out_file = Path::new(&out_dir).join("test_cases.rs");

    let test_data_dir = env::current_dir()?.join("tests/data/rules-conformance");
    let mut files = Vec::new();
    visit_dirs(&test_data_dir, &mut files);

    let mut output = fs::File::create(out_file).context("could not write test_cases.rs")?;

    writeln!(
        &mut output,
        "const TEST_DATA_DIR: &str = \"{}\";",
        test_data_dir.to_string_lossy().escape_default()
    )?;
    writeln!(&mut output)?;

    for path in files {
        let Ok(relative_path) = path.strip_prefix(&test_data_dir) else {
            continue;
        };
        let Some(relative_path_str) = relative_path.to_str() else {
            continue;
        };
        if relative_path_str.contains("pparams-by-hash") {
            continue;
        }
        let pparams_dir = "eras/conway/impl/dump/pparams-by-hash";
        let result = match failures.get(relative_path_str) {
            Some(reason) => format!("Err(\"{}\")", reason.escape_default()),
            None => "Ok(())".to_string(),
        };
        writeln!(
            &mut output,
            "#[test_case::test_case(\"{}\", \"{}\", {result})]",
            relative_path_str.escape_default(),
            pparams_dir.escape_default()
        )?;
    }
    writeln!(
        &mut output,
        r#"pub fn rules_conformance_test_case(snapshot: &str, pparams_dir: &str, result: Result<(), &str>) -> Result<(), Box<dyn std::error::Error>> {{
    let test_data_dir = Path::new(TEST_DATA_DIR);
    import_and_evaluate_vector(test_data_dir, snapshot, pparams_dir, result)
}}"#
    )?;

    Ok(())
}

fn visit_dirs(dir: &Path, files: &mut Vec<PathBuf>) {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                visit_dirs(&path, files);
            } else if path.is_file() {
                files.push(path);
            }
        }
    }
}
