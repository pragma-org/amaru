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

use std::{any::type_name, collections::BTreeSet, fmt, io, sync::Arc};

use parking_lot::Mutex;
use pure_stage::{
    DeserializerGuards, Effect, Name, Resources, SendData, StageGraph, TerminationReason,
    simulation::{SimulationBuilder, SimulationRunning},
    trace_buffer::{TraceBuffer, TraceEntry},
};
use tokio::runtime::Handle;
use tracing::{Level, subscriber::DefaultGuard};
use tracing_subscriber::util::SubscriberInitExt;

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

pub fn tm_state<'a, T: SendData>(
    at_stage: &'a str,
    prop: impl Fn(&T) -> bool + Send + 'a,
    property: &'a str,
) -> TraceMatch<'a> {
    TraceMatch::Property(
        Box::new(
            move |e| matches!(e, TraceEntry::State { stage, state } if stage.as_str() == at_stage && state.cast_ref::<T>().is_ok_and(&prop)),
        ),
        format!("state at {} of type {} with {}", at_stage, type_name::<T>(), property),
    )
}

pub fn te_input<T: SendData + Clone>(stage: impl AsRef<str>, msg: &T) -> TraceEntry {
    TraceEntry::Input { stage: Name::from(stage.as_ref()), input: Box::new(msg.clone()) }
}

pub fn te_send(from: impl AsRef<str>, to: impl AsRef<str>, msg: impl pure_stage::SendData) -> TraceEntry {
    TraceEntry::suspend(pure_stage::Effect::send(from, to, Box::new(msg)))
}

pub fn te_terminate(at_stage: impl AsRef<str>) -> TraceEntry {
    TraceEntry::suspend(Effect::Terminate { at_stage: Name::from(at_stage.as_ref()) })
}

pub fn te_terminated(at_stage: impl AsRef<str>, reason: TerminationReason) -> TraceEntry {
    TraceEntry::Terminated { stage: Name::from(at_stage.as_ref()), reason }
}

#[track_caller]
pub fn assert_trace(running: &SimulationRunning, expected: &[TraceEntry]) {
    let mut tb = running.trace_buffer().lock();
    let trace = tb
        .iter_entries()
        .filter_map(|(_, e)| (!matches!(e, TraceEntry::Resume { .. })).then_some(e))
        .collect::<Vec<_>>();
    tb.clear();
    pretty_assertions::assert_eq!(trace, expected);
}

/// Common simulation harness for stage unit tests.
///
/// This factors out the repetitive boilerplate of:
/// - setting up a `BufferWriter` + tracing subscriber for log capture,
/// - creating a `SimulationBuilder` with a trace buffer,
/// - installing resources and creating/wiring/preloading the stage(s),
/// - enabling virtual child stages (the recommended default per the README),
/// - installing external effect overrides, and
/// - running until blocked.
///
/// The caller provides:
/// - `guards`: the deserializers required for the stage, its messages, child stages, and any
///   external effects (typically built by a per-stage `register_guards()` function).
/// - `build_network`: a closure that receives a fresh `&mut SimulationBuilder` and is
///   responsible for installing any stage(s) (`network.stage(...)`), wiring them up,
///   and preloading input messages. Resource installation should be done via the
///   `setup_resources` closure (see below).
/// - `setup_resources`: a function that will be called with `&Resources` (from the
///   `SimulationBuilder`) so the caller can put stores, validators, etc.
/// - `setup_overrides`: a function that will be called with `&mut SimulationRunning` after
///   the network has started (and virtual child stages have been enabled). Use it to call
///   `running.override_external_effect::<T>(...)`.
#[track_caller]
pub fn run_simulation<F, G>(
    rt: &Handle,
    guards: DeserializerGuards,
    build_network: impl FnOnce(&mut SimulationBuilder),
    setup_resources: F,
    setup_overrides: G,
) -> (SimulationRunning, DeserializerGuards, Logs)
where
    F: FnOnce(&Resources),
    G: FnOnce(&mut SimulationRunning),
{
    let writer = BufferWriter::new();
    let mut logs = writer.clone();

    let sub = tracing_subscriber::fmt()
        .with_max_level(Level::DEBUG)
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .set_default();
    logs.set_guard(sub);

    let mut network = SimulationBuilder::default().with_trace_buffer(TraceBuffer::new_shared(100, 1000000));

    setup_resources(network.resources());

    build_network(&mut network);

    let mut running = network.run();
    running.use_virtual_child_stages(true);
    setup_overrides(&mut running);
    running.run_until_blocked_incl_effects(rt);

    (running, guards, logs.logs())
}

// Re-export TraceMatch (the type) so stage test_setup modules can use it without reaching into pure_stage.
pub use pure_stage::TraceMatch;
