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
//
use std::{
    fmt::Display,
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{self, Command},
};

use amaru_observability::{debug, warn};
use anyhow::Context;

pub struct ProcessIdHandle {
    path: PathBuf,
    pid: u32,
}

impl ProcessIdHandle {
    pub fn new<P: AsRef<Path>>(path: P) -> anyhow::Result<Self> {
        let path = path.as_ref().to_path_buf();
        let pid = process::id();

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create PID file directory {}", parent.display()))?;
        }

        let mut file = match fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => file,
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                if let Some(existing_pid) =
                    fs::read_to_string(&path).ok().and_then(|content| content.trim().parse::<u32>().ok())
                {
                    if process_exists(existing_pid) {
                        anyhow::bail!(
                            "process {existing_pid} is already running. Consider using a different PID file."
                        );
                    }
                    fs::remove_file(&path)
                        .with_context(|| format!("failed to replace stale PID file {}", path.display()))?;
                    fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&path)
                        .with_context(|| format!("failed to create PID file {}", path.display()))?
                } else {
                    return Err(err).with_context(|| format!("failed to create PID file {}", path.display()));
                }
            }
            Err(err) => return Err(err).with_context(|| format!("failed to create PID file {}", path.display())),
        };

        write!(file, "{pid}").with_context(|| format!("failed to write PID file {}", path.display()))?;
        Ok(Self { path, pid })
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }
}

impl Drop for ProcessIdHandle {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

impl Display for ProcessIdHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.path.display())
    }
}

#[cfg(unix)]
pub fn process_exists(pid: u32) -> bool {
    Command::new("kill").args(["-0", &pid.to_string()]).output().map(|output| output.status.success()).unwrap_or(false)
}

#[cfg(windows)]
pub fn process_exists(pid: u32) -> bool {
    Command::new("tasklist")
        .args(["/FI", &format!("PID eq {}", pid)])
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).contains(&pid.to_string()))
        .unwrap_or(false)
}

/// Create a PID file if a path is provided. On failure, logs a warning and returns `None`
/// so the process can continue without a PID file.
///
/// Keep the returned handle in scope for the lifetime of the process; dropping it removes the file.
pub fn optional_pid_file(maybe_path: Option<impl AsRef<Path>>) -> Option<ProcessIdHandle> {
    maybe_path.and_then(|path| {
        ProcessIdHandle::new(path)
            .inspect(|pid_file| {
                debug!(setup::pid::CREATED, path = pid_file.to_string(), pid = pid_file.pid());
            })
            .inspect_err(|e| {
                warn!(setup::pid::WRITE_FAILED, error = e.to_string());
            })
            .ok()
    })
}
