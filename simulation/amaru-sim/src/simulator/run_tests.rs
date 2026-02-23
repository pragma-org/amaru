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

use std::{fs::create_dir_all, iter::once, path::Path, sync::Arc, time::SystemTime};

use amaru::tests::{
    configuration::{
        NodeTestConfig,
        NodeType::{DownstreamNode, NodeUnderTest, UpstreamNode},
    },
    setup::create_nodes,
};
use amaru_consensus::headers_tree::data_generation::{Action, GeneratedActions, shrink};
use amaru_kernel::{BlockHeader, Peer};
use pure_stage::trace_buffer::TraceBuffer;

use crate::simulator::{
    Args, RunConfig, TestResult,
    checks::check_chain_property,
    generate_actions,
    report::{create_symlink_dir, display_actions_statistics, persist_args, persist_generated_data, persist_traces},
};

/// Run the tests simulating the execution of several nodes based on the given arguments.
pub fn run_tests(args: Args) -> anyhow::Result<()> {
    let run_config = RunConfig::from(args.clone());
    tracing::info!(?run_config, "simulate.start");

    let tests_dir = run_config.test_data_dir.as_path();
    if !tests_dir.exists() {
        create_dir_all(tests_dir)?;
    }

    let now = SystemTime::now();
    let test_run_name = format!("{}", now.duration_since(SystemTime::UNIX_EPOCH)?.as_secs());
    let test_run_dir = tests_dir.join(test_run_name);
    create_dir_all(&test_run_dir)?;
    create_symlink_dir(test_run_dir.as_path(), tests_dir.join("latest").as_path());
    persist_args(test_run_dir.as_path(), &args, args.persist_on_success)?;

    for test_number in 1..=run_config.number_of_tests {
        run_test_nb(&run_config, test_run_dir.as_path(), test_number)?;
    }

    Ok(())
}

/// Run one test and output the results in the `test_run_dir` directory.
fn run_test_nb(run_config: &RunConfig, test_run_dir: &Path, test_number: u32) -> anyhow::Result<()> {
    let test_run_dir_n = test_run_dir.join(format!("test-{}", test_number));
    create_dir_all(&test_run_dir_n)?;
    create_symlink_dir(test_run_dir_n.as_path(), test_run_dir_n.parent().unwrap().join("latest").as_path());

    tracing::info!(
        test_number, total=%run_config.number_of_tests,
        "simulate.generate_test_data"
    );

    let actions = generate_actions(run_config);
    display_actions_statistics(&actions);

    let test_result = run_test(run_config, &actions);
    if test_result.is_ok() {
        tracing::info!("the test {test_number} is successful");
    }

    let persist = run_config.persist_on_success || test_result.is_err();
    persist_generated_data(&test_run_dir_n, &actions, persist)?;
    if let Ok(trace_buffer) = test_result.get_node_under_test_trace_buffer() {
        persist_traces(test_run_dir_n.as_path(), trace_buffer, persist)?;
    };
    test_result.finalize_result(run_config, test_number)
}

/// Create a series of nodes and run one test on that system with generated data.
pub fn run_test(run_config: &RunConfig, actions: &GeneratedActions) -> TestResult {
    let test = |actions: &GeneratedActions| {
        let mut rng = run_config.rng();
        let mut nodes = create_nodes(&mut rng, node_configs(run_config, actions)).expect("failed to create nodes");

        // Scale steps based on number of peers (more peers = more stages = more effects)
        let base_steps = 10000;
        let per_peer_steps = 2000;
        let total_peers = run_config.number_of_upstream_peers as usize + run_config.number_of_downstream_peers as usize;
        let steps = base_steps + (total_peers * per_peer_steps);

        nodes.run(&mut rng, steps);
        check_chain_property(nodes, actions)
    };

    if run_config.enable_shrinking {
        let (test_result, _shrunk_actions, number_of_shrinks) =
            shrink(&test, actions, |test_result: &TestResult| test_result.is_err());
        test_result.set_number_of_shrinks(number_of_shrinks)
    } else {
        test(actions)
    }
}

/// Create the configurations for the nodes forming the whole system:
///
///  - There are upstream nodes. They are loaded with generated data for sending chainsync messages.
///  - There is one node under test.
///  - There are downstream nodes
///
pub fn node_configs(run_config: &RunConfig, actions: &GeneratedActions) -> Vec<NodeTestConfig> {
    let upstream_peers = run_config.upstream_peers();

    let upstream_nodes = upstream_peers
        .iter()
        .map(|peer| {
            NodeTestConfig::default()
                .with_no_upstream_peers()
                .with_listen_address(&peer.name)
                .with_chain_length(run_config.generated_chain_depth)
                .with_actions(get_peer_actions(actions, peer))
                .with_validated_blocks(get_headers(actions, peer))
                .with_node_type(UpstreamNode)
        })
        .collect::<Vec<_>>();

    let listen_address = "127.0.0.1:3000";
    let trace_buffer = Arc::new(parking_lot::Mutex::new(TraceBuffer::new(42, 1_000_000_000)));
    let node_under_test = NodeTestConfig::default()
        .with_listen_address(listen_address)
        .with_chain_length(run_config.generated_chain_depth)
        .with_upstream_peers(upstream_peers)
        .with_trace_buffer(trace_buffer)
        .with_validated_blocks(vec![actions.get_anchor()])
        .with_node_type(NodeUnderTest);

    let downstream_nodes = run_config
        .downstream_peers()
        .iter()
        .map(|peer| {
            NodeTestConfig::default()
                .with_listen_address(&peer.name)
                .with_chain_length(run_config.generated_chain_depth)
                .with_upstream_peer(Peer::new(listen_address))
                .with_validated_blocks(vec![actions.get_anchor()])
                .with_node_type(DownstreamNode)
        })
        .collect::<Vec<_>>();

    upstream_nodes.into_iter().chain(once(node_under_test)).chain(downstream_nodes).collect()
}

/// Extract all the actions to be executed by a given peer
fn get_peer_actions(actions: &GeneratedActions, peer: &Peer) -> Vec<Action> {
    actions.actions_per_peer().get(peer).into_iter().flatten().cloned().collect::<Vec<_>>()
}

/// Extract all the block headers forwarded by a given peer
fn get_headers(actions: &GeneratedActions, peer: &Peer) -> Vec<BlockHeader> {
    get_peer_actions(actions, peer)
        .into_iter()
        .filter_map(|action| match action {
            Action::RollForward { header, .. } => Some(header),
            Action::Rollback { .. } => None,
        })
        .collect::<Vec<_>>()
}
