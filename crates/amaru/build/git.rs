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

use std::{
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Result, bail};

use crate::{emit_rerun_if_exists, write_if_changed};

pub(crate) fn write_git_info_file(workspace_dir: &Path, out_dir: &Path) -> Result<()> {
    emit_git_rerun_paths(workspace_dir);

    let commit_hash_short = git_stdout(workspace_dir, &["rev-parse", "--short", "HEAD"]);
    let commit_hash = git_stdout(workspace_dir, &["rev-parse", "HEAD"]);
    let dirty = git_dirty(workspace_dir)?;
    let output = out_dir.join("git_info.rs");
    let contents = format!(
        "pub const GIT_COMMIT_HASH_SHORT: Option<&str> = {};\npub const GIT_COMMIT_HASH: Option<&str> = {};\npub const GIT_DIRTY: Option<bool> = {};\n",
        option_string_literal(commit_hash_short.as_deref()),
        option_string_literal(commit_hash.as_deref()),
        option_bool_literal(dirty),
    );

    write_if_changed(&output, &contents)
}

fn emit_git_rerun_paths(workspace_dir: &Path) {
    emit_git_path_rerun(workspace_dir, "HEAD");
    emit_git_path_rerun(workspace_dir, "packed-refs");

    if let Some(reference) = git_stdout(workspace_dir, &["symbolic-ref", "-q", "HEAD"]) {
        emit_git_path_rerun(workspace_dir, &reference);
    }
}

fn emit_git_path_rerun(workspace_dir: &Path, name: &str) {
    if let Some(path) = git_path(workspace_dir, name) {
        emit_rerun_if_exists(&path);
    }
}

fn git_path(workspace_dir: &Path, name: &str) -> Option<PathBuf> {
    let path = git_stdout(workspace_dir, &["rev-parse", "--git-path", name])?;
    let path = Path::new(&path);
    Some(if path.is_absolute() { path.to_path_buf() } else { workspace_dir.join(path) })
}

/// Run a git command in `workspace_dir` and return its trimmed standard output,
/// or `None` if the command fails or prints nothing.
fn git_stdout(workspace_dir: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).current_dir(workspace_dir).output().ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8(output.stdout).ok()?;
    let stdout = stdout.trim_end();

    (!stdout.is_empty()).then(|| stdout.to_string())
}

fn git_dirty(workspace_dir: &Path) -> Result<Option<bool>> {
    let output = Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=no"])
        .current_dir(workspace_dir)
        .output()?;

    if !output.status.success() {
        bail!("git status failed ({}): {}", output.status, String::from_utf8_lossy(&output.stderr));
    }

    Ok(Some(!output.stdout.is_empty()))
}

fn option_string_literal(value: Option<&str>) -> String {
    value.map(|value| format!("Some({value:?})")).unwrap_or_else(|| "None".to_owned())
}

fn option_bool_literal(value: Option<bool>) -> &'static str {
    match value {
        Some(true) => "Some(true)",
        Some(false) => "Some(false)",
        None => "None",
    }
}
