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

use amaru::tests::configuration::NodeConfig;
use amaru_sim::simulator::{
    Args, RunConfig, TEST_DATA_DIR,
    run::{replay, run},
};
use anyhow::anyhow;
use pure_stage::{Instant, serde::from_cbor, trace_buffer::TraceEntry};
use std::{
    env,
    fmt::{Display, Formatter},
    fs,
    path::Path,
    str::FromStr,
};
use tracing_subscriber::EnvFilter;

mod traces;

/// Run the simulator with arguments from environment variables.
#[test]
pub fn run_simulator() {
    initialize_logs();
    run(make_args());
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

/// Test the simulation with default parameters and replay the resulting trace
#[test]
fn test_run_replay() {
    let mut args = make_args();
    args.persist_on_success = true;
    args.number_of_tests = 1;
    args.persist_directory = format!("{TEST_DATA_DIR}/run_replay");
    run(args.clone());
    let traces = get_traces(
        Path::new(&args.persist_directory),
        SimulationRun::Latest,
        TestRun::Latest,
    )
    .expect("latest traces");
    replay(args, traces).unwrap();
}

/// Initialize logging based on environment variables:
///
///  - `AMARU_SIMULATION_LOG`: sets the log filter (default: none)
///  - `AMARU_SIMULATION_LOG_AS_JSON`: if set to "1" or "true", logs are formatted as JSON
///
fn initialize_logs() {
    let amaru_logs = get_env_var::<String>("AMARU_SIMULATION_LOG", "".to_string());
    let amaru_logs_as_json = is_true("AMARU_SIMULATION_LOG_AS_JSON");
    let formatter = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            EnvFilter::builder()
                .parse(format!("none,{}", amaru_logs))
                .unwrap_or_else(|e| panic!("invalid AMARU_SIMULATION_LOG filter: {e}")),
        );
    if amaru_logs_as_json {
        formatter.json().init();
    } else {
        formatter.init();
    }
}

/// Create Args from environment variables, with defaults from SimulateConfig and NodeConfig.
fn make_args() -> Args {
    let run_config = RunConfig::default();
    let node_config = NodeConfig::default();

    Args {
        number_of_tests: get_env_var("AMARU_NUMBER_OF_TESTS", run_config.number_of_tests),
        number_of_upstream_peers: get_env_var(
            "AMARU_NUMBER_OF_UPSTREAM_PEERS",
            run_config.number_of_upstream_peers,
        ),
        number_of_downstream_peers: get_env_var(
            "AMARU_NUMBER_OF_DOWNSTREAM_PEERS",
            run_config.number_of_downstream_peers,
        ),
        generated_chain_depth: get_env_var(
            "AMARU_GENERATED_CHAIN_DEPTH",
            node_config.chain_length as u64,
        ),
        disable_shrinking: is_true_or("AMARU_DISABLE_SHRINKING", run_config.disable_shrinking),
        seed: get_optional_env_var("AMARU_TEST_SEED"),
        persist_on_success: is_true_or("AMARU_PERSIST_ON_SUCCESS", run_config.persist_on_success),
        persist_directory: get_env_var(
            "AMARU_PERSIST_DIRECTORY",
            run_config.persist_directory.to_string_lossy().to_string(),
        ),
    }
}

/// Specify which simulation output to load.
#[allow(dead_code)]
enum SimulationRun {
    Latest,
    Timestamp(String),
}

impl Display for SimulationRun {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            SimulationRun::Latest => write!(f, "latest"),
            SimulationRun::Timestamp(ts) => write!(f, "{}", ts),
        }
    }
}

/// Specify which test run output to load.
#[allow(dead_code)]
enum TestRun {
    Latest,
    Number(u64),
}

impl Display for TestRun {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            TestRun::Latest => write!(f, "latest"),
            TestRun::Number(n) => write!(f, "test-{}", n),
        }
    }
}

/// Load the Args from the test output directory for a given simulation run.
fn get_args(test_directory: &Path, simulation_run: SimulationRun) -> anyhow::Result<Args> {
    let path = format!("{}/{simulation_run}/args.json", test_directory.display());
    let path = Path::new(&path);
    let path = fs::canonicalize(path)
        .map_err(|e| anyhow!("cannot canonicalize the file at {path:?}: {e}"))?;
    let data = fs::read(&path).map_err(|e| anyhow!("cannot read the file at {path:?}: {e}"))?;
    let args: Args = serde_json::from_slice(data.as_slice())?;
    Ok(args)
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

// Parse the environment variable `var_name` as type T, or return `default` if not set or invalid.
fn get_env_var<T: FromStr>(var_name: &str, default: T) -> T {
    env::var(var_name)
        .ok()
        .and_then(|v| v.parse::<T>().ok())
        .unwrap_or(default)
}

// Parse the environment variable `var_name` as Some(T), or return None if not set or invalid.
fn get_optional_env_var<T: FromStr>(var_name: &str) -> Option<T> {
    env::var(var_name).ok().and_then(|v| v.parse::<T>().ok())
}

/// Return true if the environment variable `var_name` is set to "1" or "true".
fn is_true(var_name: &str) -> bool {
    env::var(var_name).is_ok_and(is_true_value)
}

/// Return true if the environment variable `var_name` is set to "1" or "true".
/// Return the default value if the variable is not set.
fn is_true_or(var_name: &str, default_value: bool) -> bool {
    env::var(var_name)
        .ok()
        .map(is_true_value)
        .unwrap_or(default_value)
}

/// Return true is the given string value represents a true value.
fn is_true_value(value: String) -> bool {
    ["1", "true", "y", "yes"].contains(&value.as_str())
}
