use amaru_kernel::Hash;
use amaru_sim::simulator::{self, Args};

#[test]
fn run_simulator() {
    let args = Args {
        stake_distribution_file: "tests/data/stake-distribution.json".into(),
        consensus_context_file: "tests/data/consensus-context.json".into(),
        chain_dir: ".".into(),
        block_tree_file: "tests/data/chain.json".into(),
        start_header: Hash::from([0; 32]),
    };

    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .json()
        .init();

    let rt = tokio::runtime::Runtime::new().unwrap();
    simulator::run(rt, args);
}
