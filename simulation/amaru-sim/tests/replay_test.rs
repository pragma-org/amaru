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

use amaru_sim::simulator::{
    RunConfig, SimulationRun, TestRun, get_args, initialize_logs, make_args, replay,
};
use amaru_sim::simulator::{TEST_DATA_DIR, run_tests};
use anyhow::anyhow;
use pure_stage::Instant;
use pure_stage::serde::from_cbor;
use pure_stage::trace_buffer::TraceEntry;
use std::fs;
use std::path::Path;

/// Test the simulation with default parameters and replay the resulting trace
#[test]
fn test_run_replay() {
    let mut args = make_args();
    args.persist_on_success = true;
    args.number_of_tests = 1;
    args.seed = Some(42);
    args.persist_directory = format!("{TEST_DATA_DIR}/run_replay");
    run_tests(args.clone()).unwrap();
    let traces = get_traces(
        Path::new(&args.persist_directory),
        SimulationRun::Latest,
        TestRun::Latest,
    )
    .expect("latest traces");
    replay(args, traces).unwrap();
}

/// Replay the latest simulation from the test output directory:
///
///  - Use At::Timestamp("1762271865".to_string()) to indicate a specific timestamp
///
pub fn run_replay() {
    initialize_logs();
    let run_config = RunConfig::default();
    let test_directory = run_config.persist_directory.as_path();
    let args = get_args(test_directory, SimulationRun::Latest).expect("latest arguments");
    let traces =
        get_traces(test_directory, SimulationRun::Latest, TestRun::Latest).expect("latest traces");
    replay(args, traces).unwrap();
}

/// Load the TraceEntries from the test output directory for a given simulation run and test run.
fn get_traces(
    test_directory: &Path,
    simulation_run: SimulationRun,
    test_run: TestRun,
) -> anyhow::Result<Vec<TraceEntry>> {
    let path = format!(
        "{}/{simulation_run}/{test_run}/traces.cbor",
        test_directory.display()
    );
    let latest_trace =
        fs::canonicalize(&path).map_err(|e| anyhow!("cannot read the file at {path:?}: {e}"))?;
    load_trace_entries(&latest_trace)
}

/// Load TraceEntries from the given file path.
/// They are deserialized from CBOR format.
fn load_trace_entries(path: &Path) -> anyhow::Result<Vec<TraceEntry>> {
    let data = fs::read(path).map_err(|e| anyhow!("cannot read the file at {path:?}: {e}"))?;
    let raw_entries: Vec<Vec<u8>> = from_cbor(&data)?;
    let mut entries = Vec::with_capacity(raw_entries.len());
    for raw in raw_entries {
        let entry: (Instant, TraceEntry) = from_cbor(&raw)?;
        entries.push(entry.1);
    }
    Ok(entries)
}
