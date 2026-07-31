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

#![expect(clippy::panic, clippy::expect_used)]

use std::{any::type_name, collections::BTreeSet, fmt, io, sync::Arc, time::Duration};

use amaru_kernel::{Epoch, PREPROD_ERA_HISTORY, Slot};
use amaru_pure_stage::{
    DeserializerGuards, Effect, Instant, Name, Resources, SendData, StageGraph, TerminationReason,
    simulation::{SimulationBuilder, SimulationRunning},
    trace_buffer::{TraceBuffer, TraceEntry},
};
use parking_lot::Mutex;
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
        Box::new(move |e| {
            let TraceEntry::State { stage, state } = e else {
                return false;
            };
            stage.as_str() == at_stage && state.cast_ref::<T>().is_ok_and(&prop)
        }),
        format!("state at {} of type {} with {}", at_stage, type_name::<T>(), property),
    )
}

pub fn te_input<T: SendData + Clone>(stage: impl AsRef<str>, msg: &T) -> TraceEntry {
    TraceEntry::Input { stage: Name::from(stage.as_ref()), input: Box::new(msg.clone()) }
}

pub fn te_send(from: impl AsRef<str>, to: impl AsRef<str>, msg: impl amaru_pure_stage::SendData) -> TraceEntry {
    TraceEntry::suspend(amaru_pure_stage::Effect::send(from, to, Box::new(msg)))
}

pub fn te_terminate(at_stage: impl AsRef<str>) -> TraceEntry {
    TraceEntry::suspend(Effect::Terminate { at_stage: Name::from(at_stage.as_ref()) })
}

pub fn te_clock_read(at_stage: impl AsRef<str>) -> TraceEntry {
    TraceEntry::suspend(Effect::clock(at_stage))
}

pub fn te_record_consensus_metrics(
    at_stage: impl AsRef<str>,
    metrics: amaru_metrics::consensus::ConsensusMetrics,
) -> TraceEntry {
    TraceEntry::suspend(Effect::external(
        at_stage.as_ref(),
        Box::new(amaru_protocols::metrics_effects::RecordMetricsEffect::new(
            amaru_metrics::MetricsEvent::ConsensusMetrics(metrics),
        )),
    ))
}

pub fn te_terminated(at_stage: impl AsRef<str>, reason: TerminationReason) -> TraceEntry {
    TraceEntry::Terminated { stage: Name::from(at_stage.as_ref()), reason }
}

#[track_caller]
pub fn assert_trace(running: &SimulationRunning, expected: &[TraceEntry]) {
    let mut tb = running.trace_buffer().lock();
    let trace = tb
        .iter_entries()
        // .map(|(_, e)| e) // left here for ease of debugging: comment next line instead of this to see effect responses
        .filter_map(|(_, e)| (!matches!(e, TraceEntry::Resume { .. })).then_some(e))
        .collect::<Vec<_>>();
    tb.clear();
    pretty_assertions::assert_eq!(trace, expected);
}

/// How far the simulation is driven after overrides are installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimulationRunMode {
    /// Resolve external effects and auto-advance scheduled wakeups until the graph is idle
    /// (or otherwise blocked without a pending sleep). Default for most stage tests.
    UntilBlocked,
    /// Resolve external effects but stop at the first scheduled wakeup without advancing
    /// the clock. Use when a stage would otherwise re-arm a timer forever under a frozen
    /// external world (e.g. ledger height held constant).
    UntilSleeping,
}

/// Common simulation harness for stage unit tests.
///
/// This factors out the repetitive boilerplate of:
/// - setting up a `BufferWriter` + tracing subscriber for log capture,
/// - creating a `SimulationBuilder` with a trace buffer,
/// - installing resources and creating/wiring/preloading the stage(s),
/// - enabling virtual child stages (the recommended default per the README),
/// - installing external effect overrides, and
/// - running until blocked (auto-advancing scheduled wakeups).
///
/// The caller provides:
/// - `guards`: the deserializers required for the stage, its messages, child stages, and any
///   external effects (typically built by a per-stage `register_guards()` function).
/// - `build_network`: a closure that receives a fresh `SimulationBuilder` by value (so
///   builder options like [`SimulationBuilder::with_mailbox_size`] can be applied), installs
///   stage(s) (`network.stage(...)`), wires them up, preloads input messages, and returns
///   the builder. Resource installation should be done via the `setup_resources` closure
///   (see below).
/// - `setup_resources`: a function that will be called with `&Resources` (from the
///   `SimulationBuilder`) so the caller can put stores, validators, etc.
/// - `setup_overrides`: a function that will be called with `&mut SimulationRunning` after
///   the network has started (and virtual child stages have been enabled). Use it to call
///   `running.override_external_effect::<T>(...)`.
///
/// For a different run policy, use [`run_simulation_with`].
#[track_caller]
pub fn run_simulation<F, G>(
    rt: &Handle,
    guards: DeserializerGuards,
    build_network: impl FnOnce(SimulationBuilder) -> SimulationBuilder,
    setup_resources: F,
    setup_overrides: G,
) -> (SimulationRunning, DeserializerGuards, Logs)
where
    F: FnOnce(&Resources),
    G: FnOnce(&mut SimulationRunning),
{
    run_simulation_with(rt, guards, build_network, setup_resources, setup_overrides, SimulationRunMode::UntilBlocked)
}

/// Like [`run_simulation`], but chooses how far to drive the simulation after setup.
///
/// `build_network` receives the builder by value so callers can apply builder options
/// (e.g. [`SimulationBuilder::with_mailbox_size`]) before staging/wiring/preloading.
#[track_caller]
pub fn run_simulation_with<F, G>(
    rt: &Handle,
    guards: DeserializerGuards,
    build_network: impl FnOnce(SimulationBuilder) -> SimulationBuilder,
    setup_resources: F,
    setup_overrides: G,
    mode: SimulationRunMode,
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

    let since_network_start = start_in_era().relative_time;
    let network = SimulationBuilder::default()
        .with_trace_buffer(TraceBuffer::new_shared(100, 1000000))
        .with_global_epoch_offset(since_network_start)
        .with_initial_clock(Instant::at_offset(Duration::from_secs(10), since_network_start));

    setup_resources(network.resources());

    let network = build_network(network);

    let mut running = network.run();
    running.use_virtual_child_stages(true);
    setup_overrides(&mut running);

    match mode {
        SimulationRunMode::UntilBlocked => {
            running.run_until_blocked_incl_effects(rt);
        }
        SimulationRunMode::UntilSleeping => {
            while let amaru_pure_stage::simulation::Blocked::Busy { .. } = running.run_until_sleeping_or_blocked() {
                rt.block_on(running.await_external_effect());
            }
        }
    }

    (running, guards, logs.logs())
}

#[derive(Debug, Clone, Copy)]
pub struct StartTimes {
    pub relative_time: Duration,
    pub slot: Slot,
    pub epoch: Epoch,
}

#[expect(clippy::unwrap_used)]
pub fn start_in_era() -> StartTimes {
    let summary = PREPROD_ERA_HISTORY.current_era_summary();
    // need to place the simulation within the current era
    let relative_time = summary.start.time + Duration::from_hours(1);
    let slot = PREPROD_ERA_HISTORY.relative_time_to_slot(relative_time).unwrap();
    let epoch = PREPROD_ERA_HISTORY.slot_to_epoch(slot, slot).unwrap();
    StartTimes { relative_time, slot, epoch }
}

// Re-export TraceMatch (the type) so stage test_setup modules can use it without reaching into amaru_pure_stage.
pub use amaru_pure_stage::TraceMatch;
// Re-export the external effect matchers for convenient use in stage tests.
pub use amaru_pure_stage::{tm_external_effect, tm_external_effect_match};
