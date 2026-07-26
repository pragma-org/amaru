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
mod type_aliases;

use std::{fs, path::Path};

use anyhow::{Context, Result};

/// Generate:
///  1. build-time information (via `built`)
///  2. The type aliases embedded in the `dump_schemas` command
///  3. The stake distribution test cases for each supported network.
///  4. Peer snapshots for known networks (best-effort fetch; embed if present).
fn main() -> Result<()> {
    built::write_built_file().context("Failed to acquire build-time information")?;
    type_aliases::write_type_aliases_file().context("Failed to generate embedded type aliases for dump_schemas")?;
    println!("cargo:rerun-if-env-changed=BUILT_OVERRIDE_amaru_PKG_VERSION_PATCH");

    for network in ["mainnet", "preprod", "preview"] {
        stake_distribution::write_stake_distribution_test_cases_file(network).with_context(|| {
            format!("Failed to generate embedded stake distribution test cases for network={network}")
        })?;
    }

    Ok(())
}

fn emit_rerun_if_exists(path: &Path) {
    if path.exists() {
        println!("cargo:rerun-if-changed={}", path.display());
    }
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
