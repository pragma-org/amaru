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

mod git;
mod stake_distribution;

use std::{fs, path::Path};

use anyhow::{Context, Result};

/// Generate:
///  1. build-time information (via `built`)
///  2. git commit identity for logging
///  3. stake distribution conformance test cases for each supported network
fn main() -> Result<()> {
    write_built_file().context("Failed to acquire build-time information")?;
    write_git_info_file().context("Failed to acquire git build information")?;
    println!("cargo:rerun-if-env-changed=BUILT_OVERRIDE_amaru_PKG_VERSION_PATCH");

    for network in ["mainnet", "preprod", "preview"] {
        stake_distribution::write_stake_distribution_test_cases_file(network).with_context(|| {
            format!("Failed to generate embedded stake distribution test cases for network={network}")
        })?;
    }

    Ok(())
}

fn write_built_file() -> Result<()> {
    let out_dir = std::env::var("OUT_DIR").context("OUT_DIR")?;
    let out_dir = Path::new(&out_dir);
    let built_path = out_dir.join("built.rs");
    let temp_path = out_dir.join("built.rs.tmp");

    built::write_built_file_with_opts(&temp_path)?;
    let contents = fs::read_to_string(&temp_path)?;
    let _ = fs::remove_file(&temp_path);
    write_if_changed(&built_path, &contents)
}

fn write_git_info_file() -> Result<()> {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").context("CARGO_MANIFEST_DIR")?;
    let out_dir = std::env::var("OUT_DIR").context("OUT_DIR")?;
    let workspace_dir = Path::new(&manifest_dir).join("../..");
    git::write_git_info_file(&workspace_dir, Path::new(&out_dir))
}

fn emit_rerun_if_exists(path: &Path) {
    if path.exists() {
        emit_rerun_if_changed(path)
    }
}

fn emit_rerun_if_changed(path: &Path) {
    println!("cargo:rerun-if-changed={}", path.display());
}

/// Write `contents` to `path` unless the file already holds them, to avoid
/// needlessly recompiling the code that includes the generated file.
fn write_if_changed(path: &Path, contents: &str) -> Result<()> {
    if fs::read_to_string(path).ok().as_deref() == Some(contents) {
        return Ok(());
    }
    fs::write(path, contents)?;
    Ok(())
}
