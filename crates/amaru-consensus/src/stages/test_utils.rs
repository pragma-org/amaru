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

use std::{collections::BTreeSet, fmt, io, sync::Arc};

use parking_lot::Mutex;
use pure_stage::{Name, SendData, trace_buffer::TraceEntry};
use tracing::{Level, subscriber::DefaultGuard};

pub struct BufferWriter {
    buffer: Arc<Mutex<Vec<u8>>>,
    guard: Option<DefaultGuard>,
}

impl BufferWriter {
    #[expect(clippy::new_without_default)]
    pub fn new() -> Self {
        Self { buffer: Arc::new(Mutex::new(Vec::new())), guard: None }
    }

    pub fn set_guard(&mut self, guard: DefaultGuard) {
        self.guard = Some(guard);
    }

    /// Extract a [`Logs`] container with all lines emitted during the test.
    pub fn logs(&self) -> Logs {
        let logs = String::from_utf8(self.buffer.lock().clone()).expect("log should be valid UTF-8");
        Logs::from_buffer(&logs)
    }
}

/// Parsed log entries extracted from a [`BufferWriter`], with level-aware assertion helpers.
pub struct Logs {
    entries: Vec<LogEntry>,
}

impl fmt::Display for Logs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for e in &self.entries {
            writeln!(f, "{}", e.line)?;
        }
        Ok(())
    }
}

#[derive(Clone)]
struct LogEntry {
    level: Level,
    line: String,
}

fn parse_level(line: &str) -> Level {
    let Some(word) = line.split_whitespace().nth(1) else {
        panic!("invalid log line: {:?}", line);
    };
    match word {
        "ERROR" => Level::ERROR,
        "WARN" => Level::WARN,
        "INFO" => Level::INFO,
        "DEBUG" => Level::DEBUG,
        "TRACE" => Level::TRACE,
        _ => panic!("invalid log level: {:?}", word),
    }
}

impl Logs {
    fn from_buffer(s: &str) -> Self {
        let entries = s
            .split('\n')
            .filter(|line| !line.is_empty())
            .map(|line| LogEntry { level: parse_level(line), line: line.to_string() })
            .collect();
        Self { entries }
    }

    /// Asserts that at least one log message exists at the given level containing the substring,
    /// removes the first matching message, and returns `self` for method chaining.
    #[track_caller]
    pub fn assert_and_remove(&mut self, level: Level, substring: &[&str]) -> &mut Self {
        let pos = self.entries.iter().position(|e| e.level == level && substring.iter().all(|s| e.line.contains(s)));
        match pos {
            Some(i) => {
                self.entries.remove(i);
                self
            }
            None => panic!(
                "expected log at {:?} containing {:?}; no such message found.\n\nLogs:\n{}",
                level, substring, self
            ),
        }
    }

    /// Asserts that no log messages remain at any of the given levels.
    #[track_caller]
    pub fn assert_no_remaining_at(&mut self, levels: impl IntoIterator<Item = Level>) -> &mut Self {
        let level_set: BTreeSet<_> = levels.into_iter().collect();
        let remaining: Vec<_> = self.entries.iter().filter(|e| level_set.contains(&e.level)).cloned().collect();
        if !remaining.is_empty() {
            panic!(
                "unexpected log messages at specified levels:\n\n{}\n\n(levels checked: {:?})",
                Logs { entries: remaining },
                level_set.iter().collect::<Vec<_>>()
            );
        }
        self
    }
}

impl Clone for BufferWriter {
    fn clone(&self) -> Self {
        Self { buffer: self.buffer.clone(), guard: None }
    }
}

impl io::Write for BufferWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut guard = self.buffer.lock();
        guard.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub fn te_state<T: SendData + Clone>(stage: impl AsRef<str>, state: &T) -> TraceEntry {
    TraceEntry::State { stage: Name::from(stage.as_ref()), state: Box::new(state.clone()) }
}

pub fn te_input<T: SendData + Clone>(stage: impl AsRef<str>, msg: &T) -> TraceEntry {
    TraceEntry::Input { stage: Name::from(stage.as_ref()), input: Box::new(msg.clone()) }
}
