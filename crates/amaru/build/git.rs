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
    env,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Result, bail};

use crate::{emit_rerun_if_changed, emit_rerun_if_exists};

/// Return the workspace-relative paths of the rust files under `crates/`, whether they are
/// tracked or untracked by git, but excluding ignored files.
pub(crate) fn get_workspace_rust_files(workspace_dir: &Path) -> Result<Vec<PathBuf>> {
    let output = Command::new("git")
        .args(["ls-files", "--cached", "--others", "--exclude-standard", "-z", "--", "crates/**/*.rs"])
        .current_dir(workspace_dir)
        .output()?;

    if !output.status.success() {
        bail!("git ls-files failed ({}): {}", output.status, String::from_utf8_lossy(&output.stderr));
    }

    emit_git_exclusion_rerun_paths(workspace_dir);
    Ok(split_nul_terminated_paths(&output.stdout).map(PathBuf::from).collect())
}

/// Ask cargo to rerun this build script when the files driving git's tracked/ignored
/// decisions change: the root `.gitignore`, the git index, and the exclusion files.
fn emit_git_exclusion_rerun_paths(workspace_dir: &Path) {
    emit_rerun_if_changed(&workspace_dir.join(".gitignore"));

    for name in ["index", "info/exclude"] {
        if let Some(path) = git_stdout(workspace_dir, &["rev-parse", "--git-path", name]) {
            let path = Path::new(&path);
            let path = if path.is_absolute() { path.to_path_buf() } else { workspace_dir.join(path) };
            emit_rerun_if_exists(&path);
        }
    }

    if let Some(path) = git_stdout(workspace_dir, &["config", "--get", "core.excludesFile"]) {
        emit_rerun_if_changed(&expand_home(&path));
    }
}

/// Substitute a leading `~/` with the home directory, as git allows in `core.excludesFile`.
fn expand_home(path: &str) -> PathBuf {
    path.strip_prefix("~/")
        .and_then(|relative| env::var_os("HOME").map(|home| PathBuf::from(home).join(relative)))
        .unwrap_or_else(|| PathBuf::from(path))
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

/// Split the NUL-terminated paths produced by `git ls-files -z`, skipping the empty trailer.
fn split_nul_terminated_paths(paths: &[u8]) -> impl Iterator<Item = &str> {
    paths.split(|byte| *byte == b'\0').filter(|path| !path.is_empty()).filter_map(|path| std::str::from_utf8(path).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expand_home_only_expands_tilde_prefix() {
        let home = PathBuf::from(env::var("HOME").expect("HOME should be set in tests"));

        assert_eq!(expand_home("~/.config/git/ignore"), home.join(".config/git/ignore"));
        assert_eq!(expand_home("/etc/gitignore"), PathBuf::from("/etc/gitignore"));
    }

    #[test]
    fn test_split_nul_terminated_paths_skips_trailing_empty_path() {
        let paths = split_nul_terminated_paths(b"crates/a/src/lib.rs\0crates/b/src/lib.rs\0").collect::<Vec<_>>();

        assert_eq!(paths, vec!["crates/a/src/lib.rs", "crates/b/src/lib.rs"]);
    }
}
