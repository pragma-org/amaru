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

use amaru_sim::simulator::run::{replay, run};
use amaru_sim::simulator::{Args, NodeConfig, SimulateConfig};
use anyhow::anyhow;
use pure_stage::serde::from_cbor;
use pure_stage::trace_buffer::TraceEntry;
use std::path::Path;
use std::str::FromStr;
use std::{env, fs};
use tracing_subscriber::EnvFilter;

mod traces;

#[test]
fn run_simulator() {
    initialize_logs();
    run(make_args());
}

#[test]
fn run_replay() {
    let args = get_latest_args().expect("latest arguments");
    let traces = get_latest_traces().expect("latest traces");
    replay(args, traces).unwrap();
}

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

fn make_args() -> Args {
    let simulate_config = SimulateConfig::default();
    let node_config = NodeConfig::default();

    Args {
        number_of_tests: get_env_var("AMARU_NUMBER_OF_TESTS", simulate_config.number_of_tests),
        number_of_nodes: get_env_var("AMARU_NUMBER_OF_NODES", simulate_config.number_of_nodes),
        number_of_upstream_peers: get_env_var(
            "AMARU_NUMBER_OF_UPSTREAM_PEERS",
            node_config.number_of_upstream_peers,
        ),
        number_of_downstream_peers: get_env_var(
            "AMARU_NUMBER_OF_DOWNSTREAM_PEERS",
            node_config.number_of_downstream_peers,
        ),
        generated_chain_depth: get_env_var(
            "AMARU_GENERATED_CHAIN_DEPTH",
            node_config.generated_chain_depth,
        ),
        disable_shrinking: is_true("AMARU_DISABLE_SHRINKING"),
        seed: get_optional_env_var("AMARU_TEST_SEED"),
        persist_on_success: is_true("AMARU_PERSIST_ON_SUCCESS"),
    }
}

fn get_latest_args() -> anyhow::Result<Args> {
    get_args_at("latest")
}

fn get_args_at(timestamp: &str) -> anyhow::Result<Args> {
    let path = format!("../../target/tests/{timestamp}/args.json");
    let path = Path::new(&path);
    let path =
        fs::canonicalize(path).map_err(|e| anyhow!("cannot read the file at {path:?}: {e}"))?;
    let data = fs::read(&path).map_err(|e| anyhow!("cannot read the file at {path:?}: {e}"))?;
    let args: Args = serde_json::from_slice(data.as_slice())?;
    Ok(args)
}

fn get_latest_traces() -> anyhow::Result<Vec<TraceEntry>> {
    get_traces_at("latest")
}

fn get_traces_at(timestamp: &str) -> anyhow::Result<Vec<TraceEntry>> {
    let path = format!("../../target/tests/{timestamp}/traces.cbor");
    let path = Path::new(&path);
    let latest_trace =
        fs::canonicalize(path).map_err(|e| anyhow!("cannot read the file at {path:?}: {e}"))?;
    load_trace_entries(&latest_trace)
}

fn load_trace_entries(path: &Path) -> anyhow::Result<Vec<TraceEntry>> {
    let data = fs::read(path).map_err(|e| anyhow!("cannot read the file at {path:?}: {e}"))?;
    let raw_entries: Vec<Vec<u8>> = from_cbor(&data)?;
    let mut entries = Vec::with_capacity(raw_entries.len());
    for raw in raw_entries {
        let entry: TraceEntry = from_cbor(&raw)?;
        entries.push(entry);
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
    env::var(var_name).is_ok_and(|v| v == "1" || v == "true")
}
