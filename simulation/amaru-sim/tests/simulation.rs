use std::env;

use amaru_kernel::Hash;
use amaru_sim::simulator::{self, Args};
use tracing_subscriber::EnvFilter;

#[test]
fn run_simulator() {
    let args = Args {
        stake_distribution_file: "tests/data/stake-distribution.json".into(),
        consensus_context_file: "tests/data/consensus-context.json".into(),
        chain_dir: "./chain.db".into(),
        block_tree_file: "tests/data/chain.json".into(),
        start_header: Hash::from([0; 32]),
        number_of_tests: env::var("AMARU_NUMBER_OF_TESTS")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .or(Some(50)),
        number_of_nodes: env::var("AMARU_NUMBER_OF_NODES")
            .ok()
            .and_then(|v| v.parse::<u8>().ok())
            .or(Some(1)),
        number_of_upstream_peers: env::var("AMARU_NUMBER_OF_UPSTREAM_PEERS")
            .ok()
            .and_then(|v| v.parse::<u8>().ok())
            .or(Some(2)),
        disable_shrinking: std::env::var("AMARU_DISABLE_SHRINKING").is_ok(),
        seed: std::env::var("AMARU_TEST_SEED")
            .ok()
            .and_then(|s| s.parse().ok()),
        persist_on_success: std::env::var("AMARU_PERSIST_ON_SUCCESS").is_ok(),
    };

    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            EnvFilter::builder()
                .parse(format!(
                    "none,{}",
                    env::var("AMARU_SIMULATION_LOG")
                        .ok()
                        .as_deref()
                        .unwrap_or("error")
                ))
                .unwrap_or_else(|e| panic!("invalid AMARU_SIMULATION_LOG filter: {e}")),
        )
        .json()
        .init();

    let rt = tokio::runtime::Runtime::new().unwrap();
    simulator::run(rt, args);
}
