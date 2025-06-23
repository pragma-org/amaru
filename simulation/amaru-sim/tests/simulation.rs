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
        seed: Some(10244329322600784733),
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
