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

use crate::simulator::RunConfig;
use amaru::tests::configuration::NodeTestConfig;
use anyhow::anyhow;
use clap::Parser;
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use std::path::Path;
use std::str::FromStr;
use std::{env, fs};
use tracing_subscriber::EnvFilter;

pub const TEST_DATA_DIR: &str = "test-data";

#[derive(Debug, Parser, Clone, Serialize, Deserialize)]
#[clap(name = "Amaru Simulator")]
#[clap(bin_name = "amaru-sim")]
#[clap(author, version, about, long_about = None)]
pub struct Args {
    /// Number of tests to run in simulation
    #[arg(long, default_value = "50", env = "AMARU_NUMBER_OF_TESTS")]
    pub number_of_tests: u32,

    /// Number of upstream peers to simulate
    #[arg(long, default_value = "2", env = "AMARU_NUMBER_OF_UPSTREAM_PEERS")]
    pub number_of_upstream_peers: u8,

    /// Number of downstream peers to simulate
    #[arg(long, default_value = "1", env = "AMARU_NUMBER_OF_DOWNSTREAM_PEERS")]
    pub number_of_downstream_peers: u8,

    /// Maximum depth of the generated chain for a given peer
    #[arg(long, default_value = "10", env = "AMARU_GENERATED_CHAIN_DEPTH")]
    pub generated_chain_depth: usize,

    #[arg(long, default_value = "true", env = "AMARU_ENABLE_SHRINKING")]
    pub enable_shrinking: bool,

    /// Seed for simulation testing.
    #[arg(long, env = "AMARU_TEST_SEED")]
    pub seed: Option<u64>,

    /// Persist generated data and pure-stage traces even if the test passes.
    #[arg(long, default_value = "false", env = "AMARU_PERSIST_ON_SUCCESS")]
    pub persist_on_success: bool,

    /// Directory where test data must be persisted
    #[arg(long, default_value = TEST_DATA_DIR, env = "AMARU_TEST_DATA_DIR")]
    pub persist_directory: String,
}

impl Args {
    pub fn from_configs(run_config: &RunConfig, node_config: &NodeTestConfig) -> Self {
        Self {
            number_of_tests: run_config.number_of_tests,
            number_of_upstream_peers: run_config.number_of_upstream_peers,
            number_of_downstream_peers: run_config.number_of_downstream_peers,
            generated_chain_depth: node_config.chain_length,
            enable_shrinking: run_config.enable_shrinking,
            seed: Some(run_config.seed),
            persist_on_success: run_config.persist_on_success,
            persist_directory: run_config.persist_directory.to_string_lossy().into(),
        }
    }

    pub fn seed(&self) -> u64 {
        self.seed.unwrap_or_else(|| rand::rng().random::<u64>())
    }
}

/// Create Args from environment variables, with defaults from SimulateConfig and NodeConfig.
pub fn make_args() -> Args {
    let run_config = RunConfig::default();
    let node_config = NodeTestConfig::default();

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
        generated_chain_depth: get_env_var("AMARU_GENERATED_CHAIN_DEPTH", node_config.chain_length),
        enable_shrinking: is_true_or("AMARU_ENABLE_SHRINKING", run_config.enable_shrinking),
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
pub enum SimulationRun {
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
pub enum TestRun {
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
pub fn get_args(test_directory: &Path, simulation_run: SimulationRun) -> anyhow::Result<Args> {
    let path = format!("{}/{simulation_run}/args.json", test_directory.display());
    let path = Path::new(&path);
    let path = fs::canonicalize(path)
        .map_err(|e| anyhow!("cannot canonicalize the file at {path:?}: {e}"))?;
    let data = fs::read(&path).map_err(|e| anyhow!("cannot read the file at {path:?}: {e}"))?;
    let args: Args = serde_json::from_slice(data.as_slice())?;
    Ok(args)
}

// Parse the environment variable `var_name` as type T, or return `default` if not set or invalid.
pub fn get_env_var<T: FromStr>(var_name: &str, default: T) -> T {
    env::var(var_name)
        .ok()
        .and_then(|v| v.parse::<T>().ok())
        .unwrap_or(default)
}

// Parse the environment variable `var_name` as Some(T), or return None if not set or invalid.
pub fn get_optional_env_var<T: FromStr>(var_name: &str) -> Option<T> {
    env::var(var_name).ok().and_then(|v| v.parse::<T>().ok())
}

/// Return true if the environment variable `var_name` is set to "1" or "true".
pub fn is_true(var_name: &str) -> bool {
    env::var(var_name).is_ok_and(is_true_value)
}

/// Return true if the environment variable `var_name` is set to "1" or "true".
/// Return the default value if the variable is not set.
pub fn is_true_or(var_name: &str, default_value: bool) -> bool {
    env::var(var_name)
        .ok()
        .map(is_true_value)
        .unwrap_or(default_value)
}

/// Return true is the given string value represents a true value.
pub fn is_true_value(value: String) -> bool {
    ["1", "true", "y", "yes"].contains(&value.as_str())
}

/// Initialize logging based on environment variables:
///
///  - `AMARU_SIMULATION_LOG`: sets the log filter (default: none)
///  - `AMARU_SIMULATION_LOG_AS_JSON`: if set to "1" or "true", logs are formatted as JSON
///
pub fn initialize_logs() {
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
        let _ = formatter.json().try_init();
    } else {
        let _ = formatter.try_init();
    }
}
