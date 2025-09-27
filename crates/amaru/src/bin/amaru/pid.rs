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
    path::{Path, PathBuf},
    process::{self, Command},
};

pub struct ProcessIdHandle {
    path: PathBuf,
    pid: u32,
}

impl ProcessIdHandle {
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self, Box<dyn std::error::Error>> {
        let path = path.as_ref().to_path_buf();
        let pid = process::id();

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        if path.exists()
            && let Ok(existing_pid) = fs::read_to_string(&path)
            && let Ok(existing_pid) = existing_pid.trim().parse::<u32>()
            && process_exists(existing_pid)
        {
            return Err(format!(
                "process {} is already running. Consider using a different PID file.",
                existing_pid
            )
            .into());
        };

        fs::write(&path, pid.to_string())?;

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
fn process_exists(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

#[cfg(windows)]
fn process_exists(pid: u32) -> bool {
    Command::new("tasklist")
        .args(["/FI", &format!("PID eq {}", pid)])
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).contains(&pid.to_string()))
        .unwrap_or(false)
}
