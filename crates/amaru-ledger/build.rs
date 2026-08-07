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
    get_conformance_test_vectors()?;
    get_fixture_test_cases("transaction")
}

type FailuresTable = BTreeMap<String, String>;

fn get_conformance_test_vectors() -> Result<()> {
    let vectors_path = "tests/data/rules-conformance";
    println!("cargo:rerun-if-changed={vectors_path}");

    let failures_path = "tests/data/rules-conformance.failures.toml";
    println!("cargo:rerun-if-changed={failures_path}");
    let failures = match fs::read(failures_path) {
        Ok(bytes) => toml::from_slice::<FailuresTable>(&bytes).context("could not parse failures file")?,
        Err(_) => FailuresTable::new(),
    };

    let out_dir = env::var("OUT_DIR").context("OUT_DIR not set")?;
    let out_file = Path::new(&out_dir).join("test_cases.rs");

    let test_data_dir = env::current_dir()?.join("tests").join("data").join("rules-conformance").join("eras");
    let mut files = Vec::new();
    visit_dirs(&test_data_dir, &mut files);

    let mut output = fs::File::create(out_file).context("could not write test_cases.rs")?;

    writeln!(&mut output, "const TEST_DATA_DIR: &str = \"{}\";", test_data_dir.to_string_lossy().escape_default())?;
    writeln!(&mut output)?;

    for path in files {
        let Ok(relative_path) = path.strip_prefix(&test_data_dir) else {
            continue;
        };

        let Some(relative_path_str) = relative_path.to_str() else {
            continue;
        };

        let relative_path_str = relative_path_str.replace("\\", "/");

        let result = match failures.get(&relative_path_str) {
            Some(reason) => format!("Err(\"{}\")", reason.escape_default()),
            None => "Ok(())".to_string(),
        };

        writeln!(&mut output, "#[test_case::test_case(\"{}\", {result})]", relative_path_str.escape_default())?;
    }
    writeln!(
        &mut output,
        r#"pub fn rules_conformance_test_case(snapshot: &str, result: Result<(), &str>) -> Result<(), Box<dyn std::error::Error>> {{
    import_and_evaluate_vector(Path::new(TEST_DATA_DIR), snapshot, result)
}}"#
    )?;

    Ok(())
}

fn get_fixture_test_cases(corpus: &str) -> Result<()> {
    println!("cargo:rerun-if-changed=tests/data/{corpus}");

    let out_dir = env::var("OUT_DIR").context("OUT_DIR not set")?;
    let out_file_name = format!("{}_test_cases.rs", corpus.replace('-', "_"));
    let out_file = Path::new(&out_dir).join(&out_file_name);

    let fixtures_dir = env::current_dir()?.join("tests").join("data").join(corpus);
    let mut files = Vec::new();
    visit_dirs(&fixtures_dir.join("pass"), &mut files);
    visit_dirs(&fixtures_dir.join("fail"), &mut files);
    files.sort();

    let mut output = fs::File::create(out_file).with_context(|| format!("could not write {out_file_name}"))?;

    let mut names = BTreeMap::new();
    for path in files {
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }

        let Some(case) = path
            .strip_prefix(&fixtures_dir)
            .ok()
            .map(|relative_path| relative_path.with_extension(""))
            .and_then(|relative_path| relative_path.to_str().map(|s| s.replace("\\", "/")))
        else {
            continue;
        };

        let fixture: serde_json::Value = serde_json::from_slice(&fs::read(&path)?)
            .with_context(|| format!("invalid json fixture: {}", path.display()))?;
        let name = fixture.get("title").and_then(|title| title.as_str()).unwrap_or(&case).to_string();
        if let Some(other) = names.insert(name.clone(), case.clone()) {
            anyhow::bail!("fixtures {other} and {case} have the same title: {name}");
        }

        writeln!(&mut output, "#[test_case::test_case(\"{}\"; \"{}\")]", case.escape_default(), name.escape_default())?;
    }
    writeln!(
        &mut output,
        r#"fn conformance(fixture_path: &str) {{
    run_conformance(fixture_path)
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
