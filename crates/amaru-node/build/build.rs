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

mod peer_snapshot;

use std::{fs, path::Path};

use anyhow::{Context, Result};

fn main() -> Result<()> {
    peer_snapshot::prepare_peer_snapshots().context("Failed to prepare embedded peer snapshots")?;
    Ok(())
}

/// Ask cargo to rerun this build script when `path` changes, but only if the path
/// currently exists, so that a missing optional input does not trigger reruns.
fn emit_rerun_if_changed(path: &Path) {
    println!("cargo:rerun-if-changed={}", path.display());
}

/// Ask cargo to rerun this build script when `path` changes, but only if the path
/// currently exists, so that a missing optional input does not trigger reruns.
fn emit_rerun_if_exists(path: &Path) {
    if path.exists() {
        emit_rerun_if_changed(path)
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
