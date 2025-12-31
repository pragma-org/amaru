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

use std::{collections::BTreeMap, env, fs, io::Write as _, path::Path};

use anyhow::{Context, Result, bail};

fn main() -> Result<()> {
    extract_conformance_test_vectors()
}

type FailuresTable = BTreeMap<String, String>;

fn extract_conformance_test_vectors() -> Result<()> {
    let tar_path = "tests/data/rules-conformance.tar.gz";
    println!("cargo:rerun-if-changed={tar_path}");
    let tar_file = fs::File::open(tar_path).context("could not open conformance tests tarball")?;

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

    let test_data_dir = Path::new(&out_dir).join("test-data");
    fs::create_dir_all(&test_data_dir).context("could not initialize test data dir")?;

    let mut output = fs::File::create(out_file).context("could not write test_cases.rs")?;

    writeln!(
        &mut output,
        "const TEST_DATA_DIR: &str = \"{}\";",
        test_data_dir.display()
    )?;
    writeln!(&mut output)?;

    let decoder = flate2::read::GzDecoder::new(tar_file);
    let mut archive = tar::Archive::new(decoder);
    for entry in archive.entries().context("could not read tar file")? {
        let mut entry = entry.context("could not read tar entry")?;
        if entry.header().entry_type().is_dir() {
            continue;
        }
        entry
            .unpack_in(&test_data_dir)
            .context("could not extract data")?;

        let path = entry.path()?;
        let Some(path) = path.to_str() else {
            bail!("invalid path \"{}\"", path.display());
        };
        if !path.contains("pparams-by-hash") {
            let pparams_dir = "eras/conway/impl/dump/pparams-by-hash";
            let result = match failures.get(path) {
                Some(reason) => format!("Err(\"{}\")", reason.escape_default()),
                None => "Ok(())".to_string(),
            };
            writeln!(
                &mut output,
                "#[test_case::test_case(\"{}\", \"{}\", {result})]",
                path.escape_default(),
                pparams_dir.escape_default()
            )?;
        }
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
